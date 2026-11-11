# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project status

Early-stage Kubernetes operator in Rust, built while learning `kube-rs`. Right now `src/main.rs` is a
placeholder and there are no dependencies yet. `LEARN.md` is the roadmap: it defines the planned
`Webpage` operator (CRD → ConfigMap + nginx Deployment + Service) and the step-by-step exercises
(steps 0 to 19). Read it before adding structure, and follow its planned layout:

- `src/lib.rs`: CRD types and shared code, including the kind-context guard (see Cluster safety).
  Examples can only import from the library target.
- `src/main.rs`: the operator binary.
- `examples/NN_<name>.rs`: one exercise per LEARN.md step, prefixed with the step number.
- `examples/crdgen.rs`: prints the CRD manifest as JSON.
- `tests/integration.rs`: cluster-dependent tests, marked `#[ignore]`.
- `manifests/crd.json`: the generated CRD, committed. A unit test fails when it drifts from
  `Webpage::crd()`, so regenerate it whenever the CRD types change.
- `manifests/`: also holds the RBAC and operator Deployment manifests.

## Commands

The toolchain is pinned in `rust-toolchain.toml` (1.99.1, with rustfmt and clippy). Edition 2024.

```sh
cargo build
cargo run
cargo run --example <name>                  # e.g. 03_list_pods
cargo run --example crdgen > manifests/crd.json   # regenerate the committed CRD (when added)
cargo test
cargo test <test_name_substring>            # run a single test (or a group by name)
cargo test -- --ignored                     # cluster-dependent integration tests (when added)
```

CI (`.github/workflows/ci.yml`) runs these three checks; run them before calling work done:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

## Dependencies

`LEARN.md` was last checked against kube 5.0, k8s-openapi 0.29, and schemars 1.x. kube ships a new
major version for roughly every Kubernetes release and its APIs move each time, so before naming a
kube function or signature, check the version in `Cargo.lock` against docs.rs for that version.

- Upgrade `kube` and `k8s-openapi` together, never one without the other.
- `k8s-openapi` takes exactly one Kubernetes-version feature; use `latest`.
- The `schemars` major version must match the one `kube` uses.
- Enable specific crate features rather than `full` (tokio: `macros,rt-multi-thread,time`).
- Don't add `serde_yaml`; it is deprecated. Emit JSON, which `kubectl` accepts everywhere.

## Conventions

The "Ground rules" section of `LEARN.md` is the source of truth for how code here is written. The
rules that most often affect a change:

- **Errors:** a `thiserror` enum in `src/lib.rs` and the operator. `anyhow` only in examples and
  `main`.
- **No `unwrap()` on data from the cluster.** Nearly every Kubernetes field is an `Option`, and a
  panic in a reconciler takes the whole controller down. Use `?`, `as_deref()`, and let-chains.
- **Logging:** `tracing` with structured fields, not `println!`. Examples may `println!` their
  actual output.
- **Reconciling:** level-triggered and idempotent. Build desired state in pure `desired_*`
  functions and unit-test them next to the code.
- **Writing to the cluster:** server-side apply with one constant field-manager name, sending only
  the fields the operator owns. Owner references for children in the same namespace. On its own
  resource the operator writes only `status` (and its finalizer), never `spec`.
- **Validation:** schema attributes first, then CEL rules via `KubeSchema`, then a condition set at
  reconcile time. An admission webhook is the last resort.

## Lints

`Cargo.toml` sets `unsafe_code = "forbid"` and enables `clippy::pedantic` at warn level. CI runs
clippy with `-D warnings`, so **every pedantic warning fails CI**, in examples and tests too.
Fix the warning. If an exception is truly justified, scope it to the smallest item with
`#[expect(clippy::..., reason = "...")]` rather than relaxing the lint table. Prefer `#[expect]` over
`#[allow]`: it warns when the exception is no longer needed.

## Cluster safety

kube-rs (`Client::try_default()`) and `kubectl` use the kubeconfig's current context, and the
default context on the dev machine may be a real cluster. Before running anything that talks to
Kubernetes, check `kubectl config current-context` and use only a local kind cluster
(`kind-*` context). Never apply, patch, or delete against any other context unless the user
explicitly asks.

The code enforces the same rule: `src/lib.rs` holds a guard that refuses to run unless the current
context starts with `kind-` (LEARN.md step 2). Every example and integration test that talks to the
cluster must call it first. The operator binary must not call it, because it has no kubeconfig when
it runs inside a cluster.

## Git

Development happens on branches off of `main`; CI runs on pushes to `main` and on PRs targeting `main`.
