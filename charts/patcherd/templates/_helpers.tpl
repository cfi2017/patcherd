{{/*
Expand the name of the chart.
*/}}
{{- define "patcherd.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
*/}}
{{- define "patcherd.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "patcherd.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels.
*/}}
{{- define "patcherd.labels" -}}
helm.sh/chart: {{ include "patcherd.chart" . }}
{{ include "patcherd.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels.
*/}}
{{- define "patcherd.selectorLabels" -}}
app.kubernetes.io/name: {{ include "patcherd.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Service account name.
*/}}
{{- define "patcherd.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "patcherd.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
Webhook image tag — defaults to appVersion.
*/}}
{{- define "patcherd.webhookTag" -}}
{{- default .Chart.AppVersion .Values.image.webhook.tag }}
{{- end }}

{{/*
Patcher image tag — defaults to appVersion.
*/}}
{{- define "patcherd.patcherTag" -}}
{{- default .Chart.AppVersion .Values.image.patcher.tag }}
{{- end }}

{{/*
TLS certificate secret name.
*/}}
{{- define "patcherd.tlsSecretName" -}}
{{- printf "%s-tls" (include "patcherd.fullname" .) }}
{{- end }}
