# 11. Operational CloudWatch Dashboard (Application, LLM, Infrastructure & Logs)
resource "aws_cloudwatch_dashboard" "operations" {
  dashboard_name = "${local.name_prefix}-operations"

  dashboard_body = jsonencode({
    widgets = [
      # ========================================================================
      # Section 1: API Traffic, Status Codes & Latency (Custom Namespace: diwop)
      # ========================================================================
      {
        type   = "text"
        x      = 0
        y      = 0
        width  = 24
        height = 1
        properties = {
          markdown = "## 1. API Traffic, Status Codes & Latency (`diwop`)"
        }
      },
      {
        type   = "metric"
        x      = 0
        y      = 1
        width  = 8
        height = 6
        properties = {
          region  = var.aws_region
          title   = "API Requests: /correct-transcript (by Status Class)"
          view    = "timeSeries"
          stacked = false
          stat    = "Sum"
          period  = 60
          metrics = [
            ["diwop", "http.server.request.count", "endpoint", "/correct-transcript", "status_class", "2xx", { label = "2xx (Success)", color = "#2ca02c" }],
            ["diwop", "http.server.request.count", "endpoint", "/correct-transcript", "status_class", "4xx", { label = "4xx (Client Error)", color = "#ff7f0e" }],
            ["diwop", "http.server.request.count", "endpoint", "/correct-transcript", "status_class", "5xx", { label = "5xx (Server Error)", color = "#d62728" }]
          ]
          yAxis = {
            left = {
              min = 0
            }
          }
        }
      },
      {
        type   = "metric"
        x      = 8
        y      = 1
        width  = 8
        height = 6
        properties = {
          region  = var.aws_region
          title   = "API Requests: /translate (by Status Class)"
          view    = "timeSeries"
          stacked = false
          stat    = "Sum"
          period  = 60
          metrics = [
            ["diwop", "http.server.request.count", "endpoint", "/translate", "status_class", "2xx", { label = "2xx (Success)", color = "#2ca02c" }],
            ["diwop", "http.server.request.count", "endpoint", "/translate", "status_class", "4xx", { label = "4xx (Client Error)", color = "#ff7f0e" }],
            ["diwop", "http.server.request.count", "endpoint", "/translate", "status_class", "5xx", { label = "5xx (Server Error)", color = "#d62728" }]
          ]
          yAxis = {
            left = {
              min = 0
            }
          }
        }
      },
      {
        type   = "metric"
        x      = 16
        y      = 1
        width  = 8
        height = 6
        properties = {
          region  = var.aws_region
          title   = "Endpoint Request Duration (Avg & Max in Seconds)"
          view    = "timeSeries"
          stacked = false
          period  = 60
          metrics = [
            ["diwop", "http.server.request.duration", "endpoint", "/correct-transcript", { stat = "Average", label = "/correct-transcript (Avg)", color = "#1f77b4" }],
            ["diwop", "http.server.request.duration", "endpoint", "/correct-transcript", { stat = "Maximum", label = "/correct-transcript (Max)", color = "#aec7e8" }],
            ["diwop", "http.server.request.duration", "endpoint", "/translate", { stat = "Average", label = "/translate (Avg)", color = "#9467bd" }],
            ["diwop", "http.server.request.duration", "endpoint", "/translate", { stat = "Maximum", label = "/translate (Max)", color = "#c5b0d5" }]
          ]
          yAxis = {
            left = {
              min   = 0
              label = "Seconds"
            }
          }
        }
      },

      # ========================================================================
      # Section 2: LLM Prompt & Token Telemetry (Custom Namespace: diwop)
      # ========================================================================
      {
        type   = "text"
        x      = 0
        y      = 7
        width  = 24
        height = 1
        properties = {
          markdown = "## 2. LLM Prompt & Token Telemetry (`diwop`)"
        }
      },
      {
        type   = "metric"
        x      = 0
        y      = 8
        width  = 12
        height = 6
        properties = {
          region  = var.aws_region
          title   = "Token Consumption Over Time by Prompt & Type (Sum)"
          view    = "timeSeries"
          stacked = true
          stat    = "Sum"
          period  = 60
          metrics = [
            ["diwop", "gen_ai.client.token.usage", "prompt", "correct-transcript", "token_type", "input", { label = "correct-transcript: input", color = "#1f77b4" }],
            ["diwop", "gen_ai.client.token.usage", "prompt", "correct-transcript", "token_type", "reasoning", { label = "correct-transcript: reasoning", color = "#ff7f0e" }],
            ["diwop", "gen_ai.client.token.usage", "prompt", "correct-transcript", "token_type", "output", { label = "correct-transcript: output", color = "#2ca02c" }],
            ["diwop", "gen_ai.client.token.usage", "prompt", "translate", "token_type", "input", { label = "translate: input", color = "#00506b" }],
            ["diwop", "gen_ai.client.token.usage", "prompt", "translate", "token_type", "reasoning", { label = "translate: reasoning", color = "#f59e0b" }],
            ["diwop", "gen_ai.client.token.usage", "prompt", "translate", "token_type", "output", { label = "translate: output", color = "#10b981" }],
            ["diwop", "gen_ai.client.token.usage", "prompt", "classify", "token_type", "input", { label = "classify: input", color = "#6366f1" }],
            ["diwop", "gen_ai.client.token.usage", "prompt", "classify", "token_type", "reasoning", { label = "classify: reasoning", color = "#ec4899" }],
            ["diwop", "gen_ai.client.token.usage", "prompt", "classify", "token_type", "output", { label = "classify: output", color = "#14b8a6" }]
          ]
          yAxis = {
            left = {
              min = 0
            }
          }
        }
      },
      {
        type   = "metric"
        x      = 12
        y      = 8
        width  = 12
        height = 6
        properties = {
          region  = var.aws_region
          title   = "Average Tokens per Invocation by Prompt & Type (Avg)"
          view    = "timeSeries"
          stacked = false
          stat    = "Average"
          period  = 300
          metrics = [
            ["diwop", "gen_ai.client.token.usage", "prompt", "correct-transcript", "token_type", "input", { label = "correct-transcript: input (Avg)", color = "#1f77b4" }],
            ["diwop", "gen_ai.client.token.usage", "prompt", "correct-transcript", "token_type", "output", { label = "correct-transcript: output (Avg)", color = "#2ca02c" }],
            ["diwop", "gen_ai.client.token.usage", "prompt", "translate", "token_type", "input", { label = "translate: input (Avg)", color = "#00506b" }],
            ["diwop", "gen_ai.client.token.usage", "prompt", "translate", "token_type", "reasoning", { label = "translate: reasoning (Avg)", color = "#f59e0b" }],
            ["diwop", "gen_ai.client.token.usage", "prompt", "translate", "token_type", "output", { label = "translate: output (Avg)", color = "#10b981" }],
            ["diwop", "gen_ai.client.token.usage", "prompt", "classify", "token_type", "input", { label = "classify: input (Avg)", color = "#6366f1" }],
            ["diwop", "gen_ai.client.token.usage", "prompt", "classify", "token_type", "output", { label = "classify: output (Avg)", color = "#14b8a6" }]
          ]
          yAxis = {
            left = {
              min = 0
            }
          }
        }
      },

      # ========================================================================
      # Section 3: Translation Domain & Safety Classifications (diwop)
      # ========================================================================
      {
        type   = "text"
        x      = 0
        y      = 14
        width  = 24
        height = 1
        properties = {
          markdown = "## 3. Translation Domain & Safety Classifications (`diwop`)"
        }
      },
      {
        type   = "metric"
        x      = 0
        y      = 15
        width  = 6
        height = 6
        properties = {
          region  = var.aws_region
          title   = "Translations by Alert Reason Category"
          view    = "bar"
          stacked = false
          stat    = "Sum"
          period  = 86400
          metrics = [
            ["diwop", "diwop.translate.classification.count", "classification_category", "ok", { label = "ok", color = "#22c55e" }],
            ["diwop", "diwop.translate.classification.count", "classification_category", "uncertain", { label = "info (uncertain)", color = "#3b82f6" }],
            ["diwop", "diwop.translate.classification.count", "classification_category", "medical", { label = "warning (medical)", color = "#f59e0b" }],
            ["diwop", "diwop.translate.classification.count", "classification_category", "financial", { label = "warning (financial)", color = "#d97706" }],
            ["diwop", "diwop.translate.classification.count", "classification_category", "juridical", { label = "warning (juridical)", color = "#b45309" }],
            ["diwop", "diwop.translate.classification.count", "classification_category", "authorities", { label = "warning (authorities)", color = "#fbbf24" }],
            ["diwop", "diwop.translate.classification.count", "classification_category", "safety", { label = "danger (safety)", color = "#ef4444" }],
            ["diwop", "diwop.translate.classification.count", "classification_category", "hate-speech", { label = "danger (hate-speech)", color = "#dc2626" }],
            ["diwop", "diwop.translate.classification.count", "classification_category", "youth-protection", { label = "danger (youth-protection)", color = "#b91c1c" }],
            ["diwop", "diwop.translate.classification.count", "classification_category", "injection", { label = "danger (injection)", color = "#991b1b" }]
          ]
        }
      },
      {
        type   = "metric"
        x      = 6
        y      = 15
        width  = 6
        height = 6
        properties = {
          region  = var.aws_region
          title   = "Translations by Topic"
          view    = "bar"
          stacked = false
          stat    = "Sum"
          period  = 86400
          metrics = [
            ["diwop", "diwop.translate.classification.count", "topic", "medical", { label = "medical" }],
            ["diwop", "diwop.translate.classification.count", "topic", "authorities", { label = "authorities" }],
            ["diwop", "diwop.translate.classification.count", "topic", "financial", { label = "financial" }],
            ["diwop", "diwop.translate.classification.count", "topic", "juridical", { label = "juridical" }],
            ["diwop", "diwop.translate.classification.count", "topic", "work", { label = "work" }],
            ["diwop", "diwop.translate.classification.count", "topic", "education", { label = "education" }],
            ["diwop", "diwop.translate.classification.count", "topic", "housing", { label = "housing" }],
            ["diwop", "diwop.translate.classification.count", "topic", "social", { label = "social" }],
            ["diwop", "diwop.translate.classification.count", "topic", "family", { label = "family" }],
            ["diwop", "diwop.translate.classification.count", "topic", "other", { label = "other" }]
          ]
        }
      },
      {
        type   = "metric"
        x      = 12
        y      = 15
        width  = 6
        height = 6
        properties = {
          region  = var.aws_region
          title   = "Translation vs. Formatting"
          view    = "bar"
          stacked = false
          stat    = "Sum"
          period  = 86400
          metrics = [
            ["diwop", "diwop.translate.classification.count", "classification_type", "translation", { label = "translation", color = "#00506b" }],
            ["diwop", "diwop.translate.classification.count", "classification_type", "formatting", { label = "formatting", color = "#24b6ff" }]
          ]
        }
      },
      {
        type   = "metric"
        x      = 18
        y      = 15
        width  = 6
        height = 6
        properties = {
          region  = var.aws_region
          title   = "Dialog History Length Distribution"
          view    = "bar"
          stacked = false
          stat    = "Sum"
          period  = 86400
          metrics = [
            ["diwop", "diwop.translate.history_length", "history_bucket", "0", { label = "0 messages" }],
            ["diwop", "diwop.translate.history_length", "history_bucket", "1-2", { label = "1-2 messages" }],
            ["diwop", "diwop.translate.history_length", "history_bucket", "3-5", { label = "3-5 messages" }],
            ["diwop", "diwop.translate.history_length", "history_bucket", "6-10", { label = "6-10 messages" }],
            ["diwop", "diwop.translate.history_length", "history_bucket", "11+", { label = "11+ messages" }]
          ]
        }
      },

      # ========================================================================
      # Section 4: ECS Fargate & ALB Infrastructure Health (AWS/ECS, AWS/ApplicationELB)
      # ========================================================================
      {
        type   = "text"
        x      = 0
        y      = 21
        width  = 24
        height = 1
        properties = {
          markdown = "## 4. ECS Fargate & ALB Infrastructure Health (`AWS/ECS`, `AWS/ApplicationELB`)"
        }
      },
      {
        type   = "metric"
        x      = 0
        y      = 22
        width  = 8
        height = 6
        properties = {
          region  = var.aws_region
          title   = "ECS Fargate CPU & Memory Utilization (%)"
          view    = "timeSeries"
          stacked = false
          period  = 60
          metrics = [
            ["AWS/ECS", "CPUUtilization", "ClusterName", aws_ecs_cluster.app.name, "ServiceName", aws_ecs_service.app.name, { stat = "Average", label = "CPU (Avg)", color = "#1f77b4" }],
            ["AWS/ECS", "CPUUtilization", "ClusterName", aws_ecs_cluster.app.name, "ServiceName", aws_ecs_service.app.name, { stat = "Maximum", label = "CPU (Max)", color = "#aec7e8" }],
            ["AWS/ECS", "MemoryUtilization", "ClusterName", aws_ecs_cluster.app.name, "ServiceName", aws_ecs_service.app.name, { stat = "Average", label = "Memory (Avg)", color = "#ff7f0e" }],
            ["AWS/ECS", "MemoryUtilization", "ClusterName", aws_ecs_cluster.app.name, "ServiceName", aws_ecs_service.app.name, { stat = "Maximum", label = "Memory (Max)", color = "#ffbb78" }]
          ]
          annotations = {
            horizontal = [
              {
                label = "80% Utilization Warning"
                value = 80
                color = "#d62728"
              }
            ]
          }
          yAxis = {
            left = {
              min = 0
              max = 100
            }
          }
        }
      },
      {
        type   = "metric"
        x      = 8
        y      = 22
        width  = 8
        height = 6
        properties = {
          region  = var.aws_region
          title   = "ALB Target Response Time (p50, p95, p99) & Host Health"
          view    = "timeSeries"
          stacked = false
          period  = 60
          metrics = [
            ["AWS/ApplicationELB", "TargetResponseTime", "LoadBalancer", aws_lb.app.arn_suffix, "TargetGroup", aws_lb_target_group.app.arn_suffix, { stat = "p50", label = "Response Time p50 (s)", color = "#2ca02c" }],
            ["AWS/ApplicationELB", "TargetResponseTime", "LoadBalancer", aws_lb.app.arn_suffix, "TargetGroup", aws_lb_target_group.app.arn_suffix, { stat = "p95", label = "Response Time p95 (s)", color = "#ff7f0e" }],
            ["AWS/ApplicationELB", "TargetResponseTime", "LoadBalancer", aws_lb.app.arn_suffix, "TargetGroup", aws_lb_target_group.app.arn_suffix, { stat = "p99", label = "Response Time p99 (s)", color = "#d62728" }],
            ["AWS/ApplicationELB", "HealthyHostCount", "LoadBalancer", aws_lb.app.arn_suffix, "TargetGroup", aws_lb_target_group.app.arn_suffix, { stat = "Minimum", label = "Healthy Hosts", yAxis = "right", color = "#1f77b4" }],
            ["AWS/ApplicationELB", "UnHealthyHostCount", "LoadBalancer", aws_lb.app.arn_suffix, "TargetGroup", aws_lb_target_group.app.arn_suffix, { stat = "Maximum", label = "Unhealthy Hosts", yAxis = "right", color = "#9467bd" }]
          ]
          yAxis = {
            left = {
              min   = 0
              label = "Seconds"
            }
            right = {
              min   = 0
              label = "Hosts"
            }
          }
        }
      },
      {
        type   = "metric"
        x      = 16
        y      = 22
        width  = 8
        height = 6
        properties = {
          region  = var.aws_region
          title   = "ALB HTTP Status Codes & Request Volume"
          view    = "timeSeries"
          stacked = false
          stat    = "Sum"
          period  = 60
          metrics = [
            ["AWS/ApplicationELB", "HTTPCode_Target_2XX_Count", "LoadBalancer", aws_lb.app.arn_suffix, "TargetGroup", aws_lb_target_group.app.arn_suffix, { label = "Target 2xx", color = "#2ca02c" }],
            ["AWS/ApplicationELB", "HTTPCode_Target_4XX_Count", "LoadBalancer", aws_lb.app.arn_suffix, "TargetGroup", aws_lb_target_group.app.arn_suffix, { label = "Target 4xx", color = "#ff7f0e" }],
            ["AWS/ApplicationELB", "HTTPCode_Target_5XX_Count", "LoadBalancer", aws_lb.app.arn_suffix, "TargetGroup", aws_lb_target_group.app.arn_suffix, { label = "Target 5xx", color = "#d62728" }],
            ["AWS/ApplicationELB", "HTTPCode_ELB_5XX_Count", "LoadBalancer", aws_lb.app.arn_suffix, { label = "ALB 5xx", color = "#9467bd" }]
          ]
          yAxis = {
            left = {
              min = 0
            }
          }
        }
      },

      # ========================================================================
      # Section 5: Operational Logs (CloudWatch Logs Insights)
      # ========================================================================
      {
        type   = "text"
        x      = 0
        y      = 28
        width  = 24
        height = 1
        properties = {
          markdown = "## 5. Operational Logs (`${aws_cloudwatch_log_group.app.name}`)"
        }
      },
      {
        type   = "log"
        x      = 0
        y      = 29
        width  = 12
        height = 6
        properties = {
          region = var.aws_region
          title  = "Recent Warnings & Errors (WARN / ERROR)"
          view   = "table"
          query  = "SOURCE '${aws_cloudwatch_log_group.app.name}' | filter level in [\"WARN\", \"ERROR\"] | fields @timestamp, level, target, message, endpoint, status | sort @timestamp desc | limit 50"
        }
      },
      {
        type   = "log"
        x      = 12
        y      = 29
        width  = 12
        height = 6
        properties = {
          region = var.aws_region
          title  = "Recent API & LLM Execution Logs"
          view   = "table"
          query  = "SOURCE '${aws_cloudwatch_log_group.app.name}' | filter ispresent(endpoint) | fields @timestamp, endpoint, status, topic, classification_type, classification_category, input_tokens, reasoning_tokens, output_tokens | sort @timestamp desc | limit 50"
        }
      }
    ]
  })
}
