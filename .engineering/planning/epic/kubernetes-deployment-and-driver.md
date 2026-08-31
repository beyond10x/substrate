---
format: aep.planning-md/1
id: epic:kubernetes-deployment-and-driver
kind: epic
status: proposed
title: Kubernetes deployment and namespace driver
summary: A public node-bound serving profile and a namespace-scoped driver expose truthful Kubernetes-backed execution without turning Substrate into a fleet scheduler.
owner: substrate
tags:
- devcenter
- driver
- kubernetes
relations:
- depends_on: epic:remote-serving
revision: 2
---
# Epic: Kubernetes deployment and namespace driver

## Outcome

Substrate runs in Kubernetes in two explicit profiles: a node-bound daemon profile for host-backed execution and a namespace-scoped controller/driver profile for Kubernetes-backed workspace and exec resources.

## Boundaries

The node-bound profile gives each stateful daemon one stable address and durable volume; mutation traffic is never round-robin load-balanced across independent ledgers. The namespace driver owns only configured namespaces and applies the same durable-before-dispatch contract as every driver. Fleet placement, cross-cluster scheduling, product quotas and billing remain outside Substrate.

## Delivery

A signed public Helm chart establishes the node-bound profile first. A separate entry gate then fixes RBAC, ownership, cleanup, capability facts and refusal behavior before a namespace workspace/exec slice is implemented.
