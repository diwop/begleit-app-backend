locals {
  name_prefix          = "${module.app_config.app_name}-${var.environment}"
  image_repository_url = "${var.registry_host}/${var.registry_project_name}/${local.name_prefix}"
}

module "app_config" {
  source = "../modules/app-config"

  environment                 = var.environment
  otel_exporter_otlp_endpoint = var.otel_exporter_otlp_endpoint
  otel_resource_attributes    = "deployment.environment=${var.environment}"
}

# 1. STACKIT Kubernetes Engine (SKE) Cluster Sketch
resource "stackit_ske_cluster" "app" {
  project_id = var.project_id
  name       = substr(replace(local.name_prefix, "_", "-"), 0, 11)

  node_pools = [
    {
      name               = "default"
      machine_type       = "c1.2"
      minimum            = 1
      maximum            = 3
      availability_zones = ["eu01-1"]
      os_version_min     = "3815.2.0"
      volume_size        = 20
      volume_type        = "storage_premium_perf1"
    }
  ]
}

resource "stackit_ske_kubeconfig" "app" {
  project_id   = var.project_id
  cluster_name = stackit_ske_cluster.app.name
  refresh      = true
}

# 2. Kubernetes Namespace & Secret (API_KEY injected as env var)
resource "kubernetes_namespace" "app" {
  metadata {
    name = local.name_prefix
  }
}

resource "kubernetes_secret" "api_key" {
  metadata {
    name      = "${module.app_config.app_name}-secrets"
    namespace = kubernetes_namespace.app.metadata[0].name
  }

  data = {
    API_KEY        = var.api_key
    GEMINI_API_KEY = var.gemini_api_key
  }

  type = "Opaque"
}

# 3. Cloud-Agnostic Container Deployment on SKE
resource "kubernetes_deployment" "app" {
  metadata {
    name      = module.app_config.app_name
    namespace = kubernetes_namespace.app.metadata[0].name
    labels = {
      app = module.app_config.app_name
    }
  }

  spec {
    replicas = 2

    strategy {
      type = "RollingUpdate"
      rolling_update {
        max_surge       = "1"
        max_unavailable = "0"
      }
    }

    selector {
      match_labels = {
        app = module.app_config.app_name
      }
    }

    template {
      metadata {
        labels = {
          app = module.app_config.app_name
        }
      }

      spec {
        security_context {
          run_as_non_root = true
          run_as_user     = 10001
          run_as_group    = 10001
        }

        container {
          name  = module.app_config.app_name
          image = "${local.image_repository_url}:${var.image_tag}"

          port {
            container_port = module.app_config.port
          }

          dynamic "env" {
            for_each = module.app_config.environment_variables
            content {
              name  = env.key
              value = env.value
            }
          }

          env {
            name = "API_KEY"
            value_from {
              secret_key_ref {
                name = kubernetes_secret.api_key.metadata[0].name
                key  = "API_KEY"
              }
            }
          }

          env {
            name = "GEMINI_API_KEY"
            value_from {
              secret_key_ref {
                name = kubernetes_secret.api_key.metadata[0].name
                key  = "GEMINI_API_KEY"
              }
            }
          }

          liveness_probe {
            http_get {
              path = module.app_config.health_check_path
              port = module.app_config.port
            }
            initial_delay_seconds = 5
            period_seconds        = 10
          }

          readiness_probe {
            http_get {
              path = module.app_config.readiness_check_path
              port = module.app_config.port
            }
            initial_delay_seconds = 3
            period_seconds        = 5
          }
        }
      }
    }
  }
}

resource "kubernetes_service" "app" {
  metadata {
    name      = module.app_config.app_name
    namespace = kubernetes_namespace.app.metadata[0].name
  }

  spec {
    selector = {
      app = module.app_config.app_name
    }

    port {
      port        = 80
      target_port = module.app_config.port
    }

    type = "LoadBalancer"
  }
}
