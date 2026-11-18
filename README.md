# `rust-operator`

A Kubernetes operator written in Rust with [kube-rs](https://kube.rs), built one step at a
time as a way of learning both.

The target is a **`Webpage` operator**. You create a `Webpage` resource containing some HTML,
and the operator serves it with nginx: it creates a ConfigMap, a Deployment, and a Service,
keeps them in sync with the resource, and reports status back on it.

```yaml
apiVersion: demo.ephemeral-roles.net/v1alpha1
kind: Webpage
metadata:
  name: hello
spec:
  replicas: 2
  html: |
    <!doctype html>
    <html><body><h1>Hello from a custom resource</h1></body></html>
```

## Status

Steps 0–10 of [`LEARN.md`](LEARN.md) are done (see its progress checklist). What exists today:

- **`Webpage` CRD** (`demo.ephemeral-roles.net/v1alpha1`), defined in `src/lib.rs` with a schema
  default for `replicas` and a CEL rule requiring `html` to be a full document. The generated
  manifest is committed as `manifests/crd.json`, and a unit test fails if it drifts.
- **Shared library code**: `thiserror` error types, a `tracing` setup, the kind-context guard
  that refuses to talk to any cluster whose context doesn't start with `kind-`, and pure
  `desired_*` builders for the child resources, each with a unit test.
- **Exercises** for steps 2–7 in `examples/`: connecting, listing pods, server-side apply of
  a ConfigMap, the dynamic API, watchers, and reflectors. `crdgen` prints the CRD and
  `wpgen` prints a sample `Webpage`.
- **The controller** in `src/main.rs`. For each `Webpage` it server-side applies a ConfigMap
  holding the HTML, an nginx Deployment, and a Service, all owned by the `Webpage` so they
  are garbage-collected with it. It watches those children (filtered by a managed-by label)
  and writes `status.readyReplicas` and `status.observedGeneration`. It runs in-cluster or,
  outside a cluster, against a kind context only.

Not done yet: status conditions, Events, finalizers, and related-resource watches (steps 11–14). Nothing from the
production-readiness phase (steps 15–19) exists yet: no
integration tests, metrics, RBAC, or operator Deployment manifests.

To try the controller on kind:

```sh
make crd                                  # regenerate manifests/crd.json
kubectl apply -f manifests/crd.json
cargo run                                 # the operator
cargo run --example wpgen | kubectl apply -f -
kubectl get webpage,configmap,deployment,service   # the Webpage and its children
```

## Roadmap

| Phase                        | Steps | What it covers                                                                                           |
|------------------------------|-------|----------------------------------------------------------------------------------------------------------|
| 0. Foundations               | 0–1   | Async Rust and Kubernetes basics, a local kind cluster, dependencies                                     |
| 1. Talking to the API server | 2–5   | Connecting, reading typed resources, server-side apply, the dynamic API                                  |
| 2. Watching for changes      | 6–7   | Watchers, reflectors, and cached stores                                                                  |
| 3. Custom resources          | 8     | The `Webpage` CRD, schema defaults, and CEL validation                                                   |
| 4. Building the operator     | 9–14  | The controller, child resources, status and conditions, errors and Events, finalizers, related resources |
| 5. Production readiness      | 15–19 | Testing, observability, packaging and RBAC, tuning, CI and supply chain                                  |

`LEARN.md` also has a **Ground rules** section: the Rust and operator practices every step is
held to, such as typed errors, no `unwrap()` on cluster data, idempotent reconciles, and
server-side apply.

The roadmap was last checked against kube 5.0, k8s-openapi 0.29 (Kubernetes 1.37), and
schemars 1.x.

## Getting started

You need:

- [rustup](https://rustup.rs). The toolchain is pinned in `rust-toolchain.toml` (Rust 1.99.1
  with rustfmt and clippy) and is installed automatically.
- Docker, [kind](https://kind.sigs.k8s.io/), and `kubectl`.

```sh
kind create cluster --name learn      # switches your context to kind-learn
kubectl config current-context        # must print kind-learn
cargo build
```

`make help` lists shortcuts for the common commands. Then start at step 0 of
[`LEARN.md`](LEARN.md). Each exercise is an example you run with
`cargo run --example <name>`, for instance `cargo run --example 03_list_pods`.

> **Use a throwaway cluster.** kube-rs and `kubectl` both act on your kubeconfig's *current
> context*. Check it before running anything that writes, and only ever point this project at
> a local `kind-*` cluster.

## Layout

This is the planned layout. `tests/` and the RBAC and Deployment manifests don't exist yet.

```text
src/
  lib.rs              # Webpage CRD types and shared code (examples import this)
  main.rs             # the operator
examples/
  NN_<name>.rs        # one exercise per LEARN.md step, prefixed with the step number
  crdgen.rs           # prints the CRD manifest
tests/
  integration.rs      # cluster-dependent tests, marked #[ignore]
manifests/
  crd.json            # generated by crdgen and committed
  ...                 # RBAC and the operator Deployment
```

## Development

CI runs three checks on pushes to `main` and on pull requests. Run them before pushing:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

`unsafe` code is forbidden and `clippy::pedantic` is enabled, so every pedantic warning fails
CI, in examples and tests too.

Tests that need a cluster are marked `#[ignore]`. Once they exist, run them against kind
with `cargo test -- --ignored`.

## License

[MIT](LICENSE)
