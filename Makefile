# Set the default target of this Makefile
.PHONY: all
all:: fmt clippy test

.PHONY: check-fmt
check-fmt: ## Check code formatting
	cargo fmt -- --config imports_granularity=Item --config format_code_in_doc_comments=true --check

.PHONY: fmt
fmt: ## Format code
	cargo fmt -- --config imports_granularity=Item --config format_code_in_doc_comments=true

.PHONY: clippy
clippy: ## Run Clippy linter
	cargo clippy --all-features --all-targets

.PHONY: test
test: ## Run unit tests
	cargo nextest run --all-features
	cargo test --all-features --doc

.PHONY: doc
doc: ## Generate documentation
	RUSTDOCFLAGS="-Dwarnings --cfg=doc_cfg -Zunstable-options --generate-link-to-definition" RUSTC_BOOTSTRAP=1 cargo doc --all-features --no-deps

.PHONY: doc-open
doc-open: ## Generate and open documentation
	RUSTDOCFLAGS="--cfg=doc_cfg -Zunstable-options --generate-link-to-definition" RUSTC_BOOTSTRAP=1 cargo doc --all-features --no-deps --open

.PHONY: clean
clean: ## Clean build artifacts
	cargo clean

.PHONY: clean-all
clean-all: clean ## Clean all generated files, including those ignored by Git. Force removal.
	git clean -dXf

.PHONY: help
help: ## Show this help
	@echo "Available targets:"
	@awk 'BEGIN {FS = ":.*?## "} /^[a-zA-Z_-]+:.*?## / {printf "\033[36m%-30s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)
