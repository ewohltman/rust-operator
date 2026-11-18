# `rust-operator`

A Kubernetes operator written in Rust with [kube-rs](https://kube.rs), built one step at a
time as a way of learning both.

The target is a **`Webpage` operator**. You create a `Webpage` resource containing some HTML,
and the operator serves it with nginx at `https://<name>.ephemeral-roles.net/`. It creates a
namespace holding a ConfigMap, a Deployment, a Service, and a Gateway API HTTPRoute, keeps them in
sync with the resource, and reports status back on it.

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

Steps 0–10 of [`LEARN.md`](LEARN.md) are done (see its progress checklist), plus a side step that
packages and deploys an MVP ahead of the roadmap. What exists today:

- **`Webpage` CRD** (`demo.ephemeral-roles.net/v1alpha1`, short name `wp`), defined in
  `src/lib.rs` with a schema default for `replicas` (1, range 0–10), a 102,400-character limit
  on `html`, and a CEL rule requiring `html` to start with `<!doctype html>`. The generated
  manifest is committed as `manifests/crd.json`, and a unit test fails if it drifts.
- **Shared library code**: `thiserror` error types, a `tracing` setup, the kind-context guard
  that refuses to talk to any cluster whose context doesn't start with `kind-`, and pure
  `desired_*` builders for the namespace, the four children, and the status, each with a unit
  test.
- **Exercises** for steps 2–7 in `examples/`: connecting, listing pods, server-side apply of
  a ConfigMap, the dynamic API, watchers, and reflectors. `crdgen` prints the CRD and
  `wpgen` prints a sample `Webpage`.
- **The controller** in `src/main.rs`. For each `Webpage` named `<name>` it server-side applies,
  as field manager `rust-operator`:
  - a namespace `wp-<name>`, owned by the `Webpage` and labeled
    `ephemeral-roles.net/public-gateway=true` (so its routes can attach to the shared Gateway)
    and `linkerd.io/inject=enabled`;
  - then, concurrently and owned by that namespace, a ConfigMap holding the HTML, an nginx
    Deployment that mounts it, a Service on port 80, and an HTTPRoute for
    `<name>.ephemeral-roles.net` attached to the `public` Gateway in `envoy-gateway-system`
    (listener `https-wildcard`).

  It watches those children (filtered by a managed-by label) and writes
  `status.readyReplicas` and `status.observedGeneration`. Once the status matches the spec it
  resyncs every 5 minutes; errors retry after 5 seconds. It runs in-cluster or, outside a
  cluster, against a kind context only.
- **Packaging**: a `Dockerfile` (release build on `rust:1.99.1`, distroless runtime image) and
  manifests for the operator's namespace, a least-privilege ClusterRole (no `delete`
  anywhere), and a single-replica `Recreate` Deployment that runs as non-root with a
  read-only root filesystem.
- **A demo page**: `static/demo.html` is an interactive diagram of how the operator serves a
  page, and `static/demo.yml` wraps it in a `Webpage` named `demo`.

Known gaps in the MVP:

- The namespace is cluster-scoped but its owner, the `Webpage`, is namespaced. Kubernetes
  treats that owner reference as unresolvable, so deleting a `Webpage` leaves `wp-<name>` and
  everything in it behind. Finalizers (step 13) will fix this.
- The children are owned by the namespace rather than the `Webpage`, so `.owns()` never maps
  their changes back to the `Webpage`. If the pods aren't ready yet when it reconciles, the
  operator waits for the next change to the `Webpage` itself rather than requeueing, so
  `readyReplicas` can stay out of date.
- The child namespace is derived from the name alone, so two `Webpage`s with the same name in
  different namespaces collide.

Not done yet: status conditions, Events, finalizers, and related-resource watches (steps
11–14), and the rest of production readiness (steps 15–19): integration tests, metrics,
health probes, leader election, and CI for the image.

To try the controller on kind:

```sh
# The HTTPRoute child needs the Gateway API CRDs:
# https://gateway-api.sigs.k8s.io/guides/#installing-gateway-api
make crd                                  # regenerate manifests/crd.json
kubectl apply -f manifests/crd.json
cargo run                                 # the operator
cargo run --example wpgen | kubectl apply -f -
kubectl get webpage example               # the Webpage and its status
kubectl get configmap,deployment,service,httproute -n wp-example   # its children
```

Nothing routes `*.ephemeral-roles.net` to a kind cluster, but
`kubectl port-forward -n wp-example svc/example 8080:80` shows the page.

To deploy the operator in-cluster:

```sh
make build                                # docker image ewohltman/rust-operator:latest
docker push ewohltman/rust-operator:latest
kubectl apply -f manifests/               # CRD, namespace, RBAC, Deployment
kubectl apply -f static/demo.yml          # serves the demo page at demo.ephemeral-roles.net
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

This is the planned layout. `tests/` doesn't exist yet.

```text
src/
  lib.rs              # Webpage CRD types and shared code (examples import this)
  main.rs             # the operator
examples/
  NN_<name>.rs        # one exercise per LEARN.md step, prefixed with the step number
  crdgen.rs           # prints the CRD manifest
  wpgen.rs            # prints a sample Webpage
tests/
  integration.rs      # cluster-dependent tests, marked #[ignore]
manifests/
  crd.json            # generated by crdgen and committed
  00-namespace.yaml   # the operator's namespace
  01-rbac.yaml        # ServiceAccount, ClusterRole, ClusterRoleBinding
  02-deployment.yaml  # the operator Deployment
static/
  demo.html           # interactive diagram of the operator's flow
  demo.yml            # demo.html wrapped in a Webpage
Dockerfile            # builds the operator image
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
