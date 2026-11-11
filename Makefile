CARGO := cargo

.PHONY: help build release check fmt clippy test doc clean

default: help

help: ## Show available commands
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-15s\033[0m %s\n", $$1, $$2}'

build: ## Build the project in debug mode
	$(CARGO) build

release: ## Build an optimized release binary
	$(CARGO) build --release

check: ## Fast compile check without generating binaries
	$(CARGO) fmt --all -- --check

fmt: ## Format code with rustfmt
	$(CARGO) fmt --all

clippy: ## Run Clippy and fail on any warnings
	$(CARGO) clippy --all-targets --all-features -- -D warnings

test: ## Run all unit and integration tests
	$(CARGO) test --all-features

doc: ## Build and open project documentation
	$(CARGO) doc --no-deps --open

clean: ## Remove build artifacts
	$(CARGO) clean
