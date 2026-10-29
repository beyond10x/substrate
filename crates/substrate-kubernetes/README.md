# b10x-substrate-kubernetes

The Substrate Kubernetes namespace driver: workspaces on PersistentVolumeClaims and executions as
one-shot Pods in one namespace the daemon claims. It reads its settings from one context extension
of the operator's JSON kubeconfig, derives every object name and label from the deployment and the
resource id, proves ownership of each object by name, label and uid before it touches it, claims
the namespace with a coordination `Lease`, and refuses to open when its credential holds more than
its Role, when the namespace has no default-deny NetworkPolicy, or when Pod Security admission
admits a privileged Pod.

It creates no cluster-scoped object, runs no `kubectl` and uses no Kubernetes client crate: it
speaks to the API server through `b10x-substrate-kubernetes-client`.

This package is under development. See the
[public Substrate documentation](https://beyond10x.github.io/substrate/) for its current status.
