terraform {
  required_version = ">= 1.6.0"

  backend "s3" {
    key                         = "diwop-begleitapp/stackit/terraform.tfstate"
    region                      = "eu01"
    skip_credentials_validation = true
    skip_region_validation      = true
    skip_requesting_account_id  = true
    skip_s3_checksum            = true
    use_path_style              = true
  }

  required_providers {
    stackit = {
      source  = "stackitcloud/stackit"
      version = "~> 0.40"
    }
    kubernetes = {
      source  = "hashicorp/kubernetes"
      version = "~> 2.30"
    }
  }
}

provider "stackit" {
  default_region = var.stackit_region
}

provider "kubernetes" {
  host                   = try(yamldecode(stackit_ske_kubeconfig.app.kube_config).clusters[0].cluster.server, "")
  cluster_ca_certificate = try(base64decode(yamldecode(stackit_ske_kubeconfig.app.kube_config).clusters[0].cluster["certificate-authority-data"]), "")
  client_certificate     = try(base64decode(yamldecode(stackit_ske_kubeconfig.app.kube_config).users[0].user["client-certificate-data"]), "")
  client_key             = try(base64decode(yamldecode(stackit_ske_kubeconfig.app.kube_config).users[0].user["client-key-data"]), "")
}
