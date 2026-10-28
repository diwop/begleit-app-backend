terraform {
  required_version = ">= 1.6.0"

  backend "s3" {
    key          = "diwop-begleitapp/aws/terraform.tfstate"
    encrypt      = true
    use_lockfile = true
  }

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

provider "aws" {
  region  = var.aws_region
  profile = var.aws_profile
}
