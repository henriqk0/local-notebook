# local-notebook: a terminal NotebookLM-style app (the `rsnotebook` binary).

CARGO ?= cargo
BIN   := rsnotebook

.DEFAULT_GOAL := help
.PHONY: help build run dev test fmt fmt-check lint check install uninstall clean

help: ## Show this help
	@awk 'BEGIN {FS = ":.*## "} /^[a-zA-Z_-]+:.*## / {printf "  \033[36m%-11s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

build: ## Optimised build -> target/release/rsnotebook
	$(CARGO) build --release

run: ## Build optimised and start the app
	$(CARGO) run --release

dev: ## Start a debug build (compiles faster)
	$(CARGO) run

test: ## Run all tests
	$(CARGO) test

fmt: ## Format the code
	$(CARGO) fmt

fmt-check: ## Fail if the code is not formatted
	$(CARGO) fmt --check

lint: ## Clippy, with warnings as errors
	$(CARGO) clippy --all-targets -- -D warnings

check: fmt-check lint test ## Everything CI would run

install: ## Install the binary into ~/.cargo/bin
	$(CARGO) install --path . --locked

uninstall: ## Remove the installed binary
	$(CARGO) uninstall $(BIN)

clean: ## Delete build output
	$(CARGO) clean
