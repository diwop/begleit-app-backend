locals {
  name_prefix        = "${module.app_config.app_name}-${var.environment}"
  custom_domain_name = var.custom_domain_name != "" ? var.custom_domain_name : "${var.environment}.${var.hosted_zone_name}"
}

module "app_config" {
  source = "../modules/app-config"

  environment                 = var.environment
  otel_exporter_otlp_endpoint = var.otel_exporter_otlp_endpoint
  otel_resource_attributes    = "deployment.environment=${var.environment}"
}

# 1. Container Registry (Amazon ECR)
resource "aws_ecr_repository" "app" {
  name                 = local.name_prefix
  image_tag_mutability = "MUTABLE"

  image_scanning_configuration {
    scan_on_push = true
  }

  encryption_configuration {
    encryption_type = "AES256"
  }
}

resource "aws_ecr_lifecycle_policy" "app" {
  repository = aws_ecr_repository.app.name

  policy = jsonencode({
    rules = [
      {
        rulePriority = 1
        description  = "Keep last 10 images"
        selection = {
          tagStatus   = "any"
          countType   = "imageCountMoreThan"
          countNumber = 10
        }
        action = {
          type = "expire"
        }
      }
    ]
  })
}

# 2. Secret Management (API_KEY injected into container environment)
resource "aws_secretsmanager_secret" "api_key" {
  name        = "${local.name_prefix}/api-key"
  description = "Bearer API key for ${local.name_prefix}"
}

resource "aws_secretsmanager_secret_version" "api_key" {
  secret_id     = aws_secretsmanager_secret.api_key.id
  secret_string = var.api_key
}

# 3. Least-Privilege IAM Roles for ECS Fargate
data "aws_iam_policy_document" "ecs_tasks_assume" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRole"]

    principals {
      type        = "Service"
      identifiers = ["ecs-tasks.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "ecs_execution" {
  name               = "${local.name_prefix}-ecs-execution"
  assume_role_policy = data.aws_iam_policy_document.ecs_tasks_assume.json
}

resource "aws_iam_role_policy_attachment" "ecs_execution_managed" {
  role       = aws_iam_role.ecs_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

data "aws_iam_policy_document" "ecs_secret_read" {
  statement {
    effect    = "Allow"
    actions   = ["secretsmanager:GetSecretValue"]
    resources = [aws_secretsmanager_secret.api_key.arn]
  }
}

resource "aws_iam_role_policy" "ecs_secret_read" {
  name   = "${local.name_prefix}-read-api-key"
  role   = aws_iam_role.ecs_execution.id
  policy = data.aws_iam_policy_document.ecs_secret_read.json
}

# Unprivileged task role (application makes no AWS SDK calls)
resource "aws_iam_role" "ecs_task" {
  name               = "${local.name_prefix}-ecs-task"
  assume_role_policy = data.aws_iam_policy_document.ecs_tasks_assume.json
}

# 4. CloudWatch Log Group (ingests container stdout JSON logs)
resource "aws_cloudwatch_log_group" "app" {
  name              = "/ecs/${local.name_prefix}"
  retention_in_days = 14
}

# 5. Networking & Security Groups (Default VPC, no NAT Gateway needed)
data "aws_vpc" "default" {
  default = true
}

data "aws_subnets" "default" {
  filter {
    name   = "vpc-id"
    values = [data.aws_vpc.default.id]
  }
}

resource "aws_security_group" "alb" {
  name        = "${local.name_prefix}-alb-sg"
  description = "Allow public HTTP and HTTPS traffic to ALB"
  vpc_id      = data.aws_vpc.default.id

  ingress {
    description = "HTTP redirect"
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    description = "HTTPS traffic"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    description = "Outbound to ECS tasks"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "aws_security_group" "ecs_tasks" {
  name        = "${local.name_prefix}-ecs-tasks-sg"
  description = "Allow inbound traffic to container port exclusively from ALB"
  vpc_id      = data.aws_vpc.default.id

  ingress {
    description     = "HTTP from ALB only"
    from_port       = module.app_config.port
    to_port         = module.app_config.port
    protocol        = "tcp"
    security_groups = [aws_security_group.alb.id]
  }

  egress {
    description = "Outbound for ECR image pull and Secrets Manager"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

# 6. Route 53 & ACM Certificate (coexisting in existing parent Hosted Zone)
data "aws_route53_zone" "parent" {
  name         = var.hosted_zone_name
  private_zone = false
}

resource "aws_acm_certificate" "app" {
  domain_name       = local.custom_domain_name
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_route53_record" "cert_validation" {
  for_each = {
    for dvo in aws_acm_certificate.app.domain_validation_options : dvo.domain_name => {
      name   = dvo.resource_record_name
      record = dvo.resource_record_value
      type   = dvo.resource_record_type
    }
  }

  allow_overwrite = true
  zone_id         = data.aws_route53_zone.parent.zone_id
  name            = each.value.name
  type            = each.value.type
  ttl             = 60
  records         = [each.value.record]
}

resource "aws_acm_certificate_validation" "app" {
  certificate_arn         = aws_acm_certificate.app.arn
  validation_record_fqdns = [for record in aws_route53_record.cert_validation : record.fqdn]
}

# 7. Application Load Balancer (ALB) & Route 53 Alias
resource "aws_lb" "app" {
  name               = local.name_prefix
  internal           = false
  load_balancer_type = "application"
  security_groups    = [aws_security_group.alb.id]
  subnets            = data.aws_subnets.default.ids
}

resource "aws_lb_target_group" "app" {
  name                 = local.name_prefix
  port                 = module.app_config.port
  protocol             = "HTTP"
  target_type          = "ip"
  vpc_id               = data.aws_vpc.default.id
  deregistration_delay = 15

  health_check {
    enabled             = true
    path                = module.app_config.health_check_path
    protocol            = "HTTP"
    matcher             = "200"
    interval            = 15
    timeout             = 5
    healthy_threshold   = 2
    unhealthy_threshold = 3
  }
}

resource "aws_lb_listener" "https" {
  load_balancer_arn = aws_lb.app.arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  certificate_arn   = aws_acm_certificate_validation.app.certificate_arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.app.arn
  }
}

resource "aws_lb_listener" "http_redirect" {
  load_balancer_arn = aws_lb.app.arn
  port              = 80
  protocol          = "HTTP"

  default_action {
    type = "redirect"

    redirect {
      port        = "443"
      protocol    = "HTTPS"
      status_code = "HTTP_301"
    }
  }
}

resource "aws_route53_record" "custom_domain" {
  allow_overwrite = true
  zone_id         = data.aws_route53_zone.parent.zone_id
  name            = local.custom_domain_name
  type            = "A"

  alias {
    name                   = aws_lb.app.dns_name
    zone_id                = aws_lb.app.zone_id
    evaluate_target_health = true
  }
}

# 8. ECS Fargate Cluster, ARM64 Task Definition, and FARGATE_SPOT Service
resource "aws_ecs_cluster" "app" {
  name = local.name_prefix
}

resource "aws_ecs_cluster_capacity_providers" "app" {
  cluster_name       = aws_ecs_cluster.app.name
  capacity_providers = ["FARGATE_SPOT", "FARGATE"]

  default_capacity_provider_strategy {
    base              = 0
    weight            = 100
    capacity_provider = "FARGATE_SPOT"
  }
}

resource "aws_ecs_task_definition" "app" {
  family                   = local.name_prefix
  network_mode             = "awsvpc"
  requires_compatibilities = ["FARGATE"]
  cpu                      = var.cpu
  memory                   = var.memory
  execution_role_arn       = aws_iam_role.ecs_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "ARM64"
  }

  container_definitions = jsonencode([
    {
      name      = module.app_config.app_name
      image     = "${aws_ecr_repository.app.repository_url}:${var.image_tag}"
      essential = true
      portMappings = [
        {
          containerPort = module.app_config.port
          hostPort      = module.app_config.port
          protocol      = "tcp"
        }
      ]
      environment = [
        for k, v in module.app_config.environment_variables : {
          name  = k
          value = v
        }
      ]
      secrets = [
        {
          name      = "API_KEY"
          valueFrom = aws_secretsmanager_secret.api_key.arn
        }
      ]
      logConfiguration = {
        logDriver = "awslogs"
        options = {
          "awslogs-group"         = aws_cloudwatch_log_group.app.name
          "awslogs-region"        = var.aws_region
          "awslogs-stream-prefix" = "ecs"
        }
      }
    }
  ])
}

resource "aws_ecs_service" "app" {
  name            = local.name_prefix
  cluster         = aws_ecs_cluster.app.id
  task_definition = aws_ecs_task_definition.app.arn
  desired_count   = 1

  capacity_provider_strategy {
    capacity_provider = "FARGATE_SPOT"
    weight            = 100
    base              = 0
  }

  network_configuration {
    subnets          = data.aws_subnets.default.ids
    security_groups  = [aws_security_group.ecs_tasks.id]
    assign_public_ip = true
  }

  load_balancer {
    target_group_arn = aws_lb_target_group.app.arn
    container_name   = module.app_config.app_name
    container_port   = module.app_config.port
  }

  lifecycle {
    ignore_changes = [desired_count]
  }

  depends_on = [
    aws_lb_listener.https,
    aws_secretsmanager_secret_version.api_key,
    aws_iam_role_policy.ecs_secret_read,
  ]
}

# 9. Scheduled Scaling (scale down to 0 at 00:00, scale up to 1 at 08:00 Europe/Berlin)
resource "aws_appautoscaling_target" "ecs" {
  max_capacity       = 1
  min_capacity       = 1
  resource_id        = "service/${aws_ecs_cluster.app.name}/${aws_ecs_service.app.name}"
  scalable_dimension = "ecs:service:DesiredCount"
  service_namespace  = "ecs"
}

resource "aws_appautoscaling_scheduled_action" "scale_down_night" {
  name               = "${local.name_prefix}-scale-down-night"
  service_namespace  = aws_appautoscaling_target.ecs.service_namespace
  resource_id        = aws_appautoscaling_target.ecs.resource_id
  scalable_dimension = aws_appautoscaling_target.ecs.scalable_dimension
  schedule           = "cron(0 0 * * ? *)"
  timezone           = "Europe/Berlin"

  scalable_target_action {
    min_capacity = 0
    max_capacity = 0
  }
}

resource "aws_appautoscaling_scheduled_action" "scale_up_morning" {
  name               = "${local.name_prefix}-scale-up-morning"
  service_namespace  = aws_appautoscaling_target.ecs.service_namespace
  resource_id        = aws_appautoscaling_target.ecs.resource_id
  scalable_dimension = aws_appautoscaling_target.ecs.scalable_dimension
  schedule           = "cron(0 8 * * ? *)"
  timezone           = "Europe/Berlin"

  scalable_target_action {
    min_capacity = 1
    max_capacity = 1
  }

  depends_on = [aws_appautoscaling_scheduled_action.scale_down_night]
}

# 10. GitHub Actions OIDC Deployment Role
data "aws_caller_identity" "current" {}

data "aws_iam_openid_connect_provider" "github" {
  url = "https://token.actions.githubusercontent.com"
}

data "aws_iam_policy_document" "github_actions_assume" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRoleWithWebIdentity"]

    principals {
      type        = "Federated"
      identifiers = [data.aws_iam_openid_connect_provider.github.arn]
    }

    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:aud"
      values   = ["sts.amazonaws.com"]
    }

    condition {
      test     = "StringLike"
      variable = "token.actions.githubusercontent.com:sub"
      values   = ["repo:${var.github_repository}:ref:refs/heads/main"]
    }
  }
}

resource "aws_iam_role" "github_actions_deploy" {
  name               = "${local.name_prefix}-github-actions-deploy"
  assume_role_policy = data.aws_iam_policy_document.github_actions_assume.json
}

data "aws_iam_policy_document" "github_actions_deploy" {
  statement {
    sid       = "OpenTofuStateBucketList"
    effect    = "Allow"
    actions   = ["s3:ListBucket"]
    resources = ["arn:aws:s3:::diwop-begleitapp-tfstate-${data.aws_caller_identity.current.account_id}"]
  }

  statement {
    sid    = "OpenTofuStateObjects"
    effect = "Allow"
    actions = [
      "s3:GetObject",
      "s3:PutObject",
      "s3:DeleteObject",
    ]
    resources = ["arn:aws:s3:::diwop-begleitapp-tfstate-${data.aws_caller_identity.current.account_id}/diwop-begleitapp/aws/*"]
  }

  statement {
    sid       = "ECRAuthToken"
    effect    = "Allow"
    actions   = ["ecr:GetAuthorizationToken"]
    resources = ["*"]
  }

  statement {
    sid       = "ECRRepositoryManageAndPush"
    effect    = "Allow"
    actions   = ["ecr:*"]
    resources = [aws_ecr_repository.app.arn]
  }

  statement {
    sid    = "SecretsManagerApiKey"
    effect = "Allow"
    actions = [
      "secretsmanager:DescribeSecret",
      "secretsmanager:GetResourcePolicy",
      "secretsmanager:GetSecretValue",
      "secretsmanager:PutSecretValue",
      "secretsmanager:UpdateSecret",
      "secretsmanager:ListSecretVersionIds",
    ]
    resources = [aws_secretsmanager_secret.api_key.arn]
  }

  statement {
    sid    = "IAMRoleReadAndPass"
    effect = "Allow"
    actions = [
      "iam:GetRole",
      "iam:GetRolePolicy",
      "iam:ListRolePolicies",
      "iam:ListAttachedRolePolicies",
      "iam:PassRole",
      "iam:PutRolePolicy",
    ]
    resources = [
      aws_iam_role.ecs_execution.arn,
      aws_iam_role.ecs_task.arn,
      aws_iam_role.github_actions_deploy.arn,
    ]
  }

  statement {
    sid    = "InfrastructureDeployAndRefresh"
    effect = "Allow"
    actions = [
      "iam:GetOpenIDConnectProvider",
      "ecs:*",
      "elasticloadbalancing:*",
      "acm:DescribeCertificate",
      "acm:ListTagsForCertificate",
      "route53:GetHostedZone",
      "route53:ListHostedZones",
      "route53:ListTagsForResource",
      "route53:ListResourceRecordSets",
      "route53:GetChange",
      "route53:ChangeResourceRecordSets",
      "ec2:Describe*",
      "logs:DescribeLogGroups",
      "logs:ListTagsForResource",
      "application-autoscaling:Describe*",
      "application-autoscaling:ListTagsForResource",
      "application-autoscaling:RegisterScalableTarget",
      "application-autoscaling:PutScheduledAction",
    ]
    resources = ["*"]
  }
}

resource "aws_iam_role_policy" "github_actions_deploy" {
  name   = "${local.name_prefix}-github-actions-deploy"
  role   = aws_iam_role.github_actions_deploy.id
  policy = data.aws_iam_policy_document.github_actions_deploy.json
}
