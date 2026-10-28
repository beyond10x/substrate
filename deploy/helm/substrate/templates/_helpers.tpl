{{- define "substrate.name" -}}
{{- if gt (len .Release.Name) 40 -}}
{{- fail (printf "release name %q is longer than 40 characters; pod names and per-pod DNS names must stay DNS labels" .Release.Name) -}}
{{- end -}}
{{- .Release.Name -}}
{{- end -}}

{{- define "substrate.selectorLabels" -}}
app.kubernetes.io/name: b10x-substrate
app.kubernetes.io/instance: {{ .Release.Name | quote }}
app.kubernetes.io/component: daemon
{{- end -}}

{{- define "substrate.labels" -}}
{{ include "substrate.selectorLabels" . }}
app.kubernetes.io/managed-by: {{ .Release.Service | quote }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | quote }}
{{- end -}}

{{- define "substrate.image" -}}
{{- if .Values.image.digest -}}
{{- printf "%s@%s" .Values.image.repository .Values.image.digest -}}
{{- else if .Values.image.tag -}}
{{- printf "%s:%s" .Values.image.repository .Values.image.tag -}}
{{- else -}}
{{- fail "image.digest or image.tag must name the daemon image; a released chart pins image.digest" -}}
{{- end -}}
{{- end -}}

{{- define "substrate.podHost" -}}
{{- printf "%s-%d.%s.%s.svc.%s" .name .ordinal .name .namespace .domain -}}
{{- end -}}

{{- define "substrate.certificateNames" -}}
{{- $name := include "substrate.name" . -}}
{{- $names := list -}}
{{- range $ordinal := until (int .Values.replicas) -}}
{{- $names = append $names (include "substrate.podHost" (dict "name" $name "ordinal" $ordinal "namespace" $.Release.Namespace "domain" $.Values.clusterDomain)) -}}
{{- end -}}
{{- join "\n" $names -}}
{{- end -}}

{{- define "substrate.anyEgress" -}}
{{- $cidrs := list .Values.identity.egress.cidr -}}
{{- range .Values.daemon.gitSources -}}
{{- $cidrs = append $cidrs .cidr -}}
{{- end -}}
{{- range .Values.daemon.egressApertures -}}
{{- $cidrs = append $cidrs .cidr -}}
{{- end -}}
{{- range $cidrs -}}
{{- $cidr := toString . -}}
{{- if not (regexMatch "^[0-9A-Fa-f.:]+/[0-9]{1,3}$" $cidr) -}}
{{- fail (printf "egress destination %q is not an address range ADDRESS/PREFIX" $cidr) -}}
{{- end -}}
{{- $prefix := regexReplaceAll "^0+([0-9])" (last (splitList "/" $cidr)) "${1}" -}}
{{- if and (eq $prefix "0") (not $.Values.networkPolicy.allowAnyEgress) -}}
{{- fail (printf "egress destination %s admits every address; set networkPolicy.allowAnyEgress: true to accept it" $cidr) -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{- define "substrate.checks" -}}
{{- include "substrate.anyEgress" . -}}
{{- $labelKey := "^([a-z0-9.-]{1,253}/)?[A-Za-z0-9]([A-Za-z0-9._-]{0,61}[A-Za-z0-9])?$" -}}
{{- $labelValue := "^[A-Za-z0-9]([A-Za-z0-9._-]{0,61}[A-Za-z0-9])?$" -}}
{{- range $field, $value := dict "nodePool.label.key" .Values.nodePool.label.key "nodePool.taint.key" .Values.nodePool.taint.key -}}
{{- if not (regexMatch $labelKey (toString $value)) -}}
{{- fail (printf "%s %q is not a label key" $field (toString $value)) -}}
{{- end -}}
{{- end -}}
{{- range $field, $value := dict "nodePool.label.value" .Values.nodePool.label.value "nodePool.taint.value" .Values.nodePool.taint.value -}}
{{- if not (regexMatch $labelValue (toString $value)) -}}
{{- fail (printf "%s %q is not a label value" $field (toString $value)) -}}
{{- end -}}
{{- end -}}
{{- range $key, $value := .Values.podAnnotations -}}
{{- if or (contains "apparmor" (lower $key)) (contains "seccomp.security.alpha.kubernetes.io/" (lower $key)) -}}
{{- fail (printf "podAnnotations %q selects an AppArmor or seccomp profile; the chart states every profile in the pod spec" $key) -}}
{{- end -}}
{{- end -}}
{{- if and .Values.execution.projectQuotas.enabled (not .Values.execution.enabled) -}}
{{- fail "execution.projectQuotas.enabled requires execution.enabled: the quota executable is selected only by the execution bootstrap" -}}
{{- end -}}
{{- if and .Values.execution.projectQuotas.enabled (not .Values.persistence.projectQuotaCapable) -}}
{{- fail "execution.projectQuotas.enabled requires persistence.projectQuotaCapable: declare a storage class whose filesystem enforces project quotas" -}}
{{- end -}}
{{- if and .Values.execution.projectQuotas.enabled (not .Values.execution.projectQuotas.ids) -}}
{{- fail "execution.projectQuotas.enabled requires execution.projectQuotas.ids as START-END" -}}
{{- end -}}
{{- if and (not .Values.execution.projectQuotas.enabled) .Values.execution.projectQuotas.ids -}}
{{- fail "execution.projectQuotas.ids is set but execution.projectQuotas.enabled is false" -}}
{{- end -}}
{{- if and .Values.nodeProfiles.enabled (not .Values.nodeProfiles.namespace) -}}
{{- fail "nodeProfiles.enabled requires nodeProfiles.namespace, a namespace of its own admitted at the privileged Pod Security level" -}}
{{- end -}}
{{- if and .Values.nodeProfiles.enabled (eq .Values.nodeProfiles.namespace .Release.Namespace) -}}
{{- fail "nodeProfiles.namespace must differ from the release namespace: the node-profile installer runs in a namespace of its own" -}}
{{- end -}}
{{- $class := lookup "storage.k8s.io/v1" "StorageClass" "" .Values.persistence.storageClassName -}}
{{- if and $class (ne (default "Immediate" $class.volumeBindingMode) "WaitForFirstConsumer") -}}
{{- fail (printf "storage class %s binds volumes %s; the ledger volume must bind on first consumer so it follows the node" .Values.persistence.storageClassName (default "Immediate" $class.volumeBindingMode)) -}}
{{- end -}}
{{- end -}}
