.PHONY: help build run watch trainer-review trainer-export test lint format check coverage

help:     ## Show this help.
	@sed -ne '/@sed/!s/## //p' $(MAKEFILE_LIST)

build:    ## Build the project using Nix
	nix build

run:      ## Run the bot locally
	mkdir -p data
	cargo run --bin brickbot

watch:    ## Watch for changes and run the bot locally
	mkdir -p data
	cargo watch -x 'run --bin brickbot'

trainer-review: ## Run the trainer CLI in review mode
	mkdir -p data
	cargo run --bin trainer -- review

trainer-export: ## Export training logs using the trainer CLI
	mkdir -p data
	cargo run --bin trainer -- export

test-fast: ## Run tests quickly without coverage
	cargo test

coverage-lcov: ## Run test coverage and output lcov
	cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info

coverage-html: ## Run test coverage and output html
	cargo llvm-cov --all-features --workspace --html

lint:     ## Run clippy checks
	cargo clippy -- -D warnings

format:   ## Format the code using treefmt (mutating)
	cargo clippy --fix --all-targets --all-features --allow-dirty --allow-staged
	treefmt

format-check: ## Check formatting without mutating
	treefmt --fail-on-change

check: format-check lint test-fast ## Run non-mutating tests and lints (used as git pre-commit hook)

devenv-%:  ## Run command in devenv shell
	devenv shell -- $(MAKE) $*

nix-%:  ## Run command in devenv shell
	devenv shell -- $(MAKE) $*
