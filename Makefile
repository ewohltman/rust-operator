CARGO := cargo
DOCKER := docker

.PHONY: help build release check crd fmt clippy test doc clean

default: help

help: ## Show available commands
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-15s\033[0m %s\n", $$1, $$2}'

build: ## Build the project image
	$(DOCKER) image build -t ewohltman/rust-operator:latest .

release: ## Build an optimized release binary
	$(CARGO) build --release

check: ## Fast compile check without generating binaries
	$(CARGO) fmt --all -- --check

crd: ## Generate CRD
	mkdir -p manifests && $(CARGO) run --example crdgen > manifests/crd-new.json && mv manifests/crd-new.json manifests/crd.json

fmt: ## Format code with rustfmt
	$(CARGO) fmt --all

clippy: fmt ## Run Clippy and fail on any warnings
	$(CARGO) clippy --all-targets --all-features -- -D warnings

test: ## Run all unit and integration tests
	$(CARGO) llvm-cov --all-features

doc: ## Build and open project documentation
	$(CARGO) doc --no-deps --open

clean: ## Remove build artifacts
	$(CARGO) clean
