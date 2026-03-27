terraform {
  required_version = ">= 1.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

provider "aws" {
  region = "ap-southeast-2" # Sydney
}

# --- S3 Bucket for memoria backups ---

resource "aws_s3_bucket" "memoria_backup" {
  bucket = "memoria-backup-juzzydee"

  tags = {
    Project = "memoria"
    Purpose = "disaster-recovery"
  }
}

# Block all public access
resource "aws_s3_bucket_public_access_block" "memoria_backup" {
  bucket = aws_s3_bucket.memoria_backup.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# Versioning — belt and suspenders
resource "aws_s3_bucket_versioning" "memoria_backup" {
  bucket = aws_s3_bucket.memoria_backup.id

  versioning_configuration {
    status = "Enabled"
  }
}

# Lifecycle rules: keep 30 daily, transition older to cheaper storage, expire after 1 year
resource "aws_s3_bucket_lifecycle_configuration" "memoria_backup" {
  bucket = aws_s3_bucket.memoria_backup.id

  rule {
    id     = "backup-lifecycle"
    status = "Enabled"

    filter {} # Apply to all objects

    # Move to Infrequent Access after 30 days (cheaper storage, same durability)
    transition {
      days          = 30
      storage_class = "STANDARD_IA"
    }

    # Move to Glacier after 90 days (very cheap, slow retrieval)
    transition {
      days          = 90
      storage_class = "GLACIER"
    }

    # Delete after 365 days
    expiration {
      days = 365
    }

    # Clean up old versions after 30 days
    noncurrent_version_expiration {
      noncurrent_days = 30
    }
  }
}

# Server-side encryption
resource "aws_s3_bucket_server_side_encryption_configuration" "memoria_backup" {
  bucket = aws_s3_bucket.memoria_backup.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

# --- IAM Policy: minimal access for the backup user ---

resource "aws_iam_user_policy" "memoria_backup" {
  name = "memoria-backup-s3-access"
  user = "memoria-backup"

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "AllowBackupOperations"
        Effect = "Allow"
        Action = [
          "s3:PutObject",
          "s3:GetObject",
          "s3:ListBucket",
        ]
        Resource = [
          aws_s3_bucket.memoria_backup.arn,
          "${aws_s3_bucket.memoria_backup.arn}/*",
        ]
      }
    ]
  })
}

# --- Outputs ---

output "bucket_name" {
  value = aws_s3_bucket.memoria_backup.id
}

output "bucket_arn" {
  value = aws_s3_bucket.memoria_backup.arn
}
