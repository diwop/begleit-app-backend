output "ecr_repository_url" {
  description = "Amazon ECR repository URL for pushing container images."
  value       = aws_ecr_repository.app.repository_url
}

output "alb_dns_name" {
  description = "Public DNS name of the Application Load Balancer."
  value       = aws_lb.app.dns_name
}

output "custom_domain_url" {
  description = "Public HTTPS custom domain URL of the deployed service."
  value       = "https://${aws_route53_record.custom_domain.fqdn}"
}

output "github_actions_role_arn" {
  description = "IAM Role ARN for GitHub Actions OIDC deployment."
  value       = aws_iam_role.github_actions_deploy.arn
}

output "gemini_api_key_ssm_parameter_name" {
  description = "SSM Parameter Store name for the Gemini API key."
  value       = aws_ssm_parameter.gemini_api_key.name
}

output "api_key_ssm_parameter_name" {
  description = "SSM Parameter Store name for the Bearer API key."
  value       = aws_ssm_parameter.api_key.name
}
