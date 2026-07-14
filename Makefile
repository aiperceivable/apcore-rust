.PHONY: setup check check-ci check-chars fmt-check lint lint-full build test test-fast build-examples fmt clean

# One-time dev environment setup
setup:
	@echo "Installing apdev-rs..."
	@command -v apdev-rs >/dev/null 2>&1 || cargo install apdev-rs
	@echo "Installing cargo-nextest (per-test process isolation for the consolidated test binary)..."
	@command -v cargo-nextest >/dev/null 2>&1 || cargo install cargo-nextest --locked
	@echo "Installing git pre-commit hook..."
	@mkdir -p .git/hooks
	@cp .githooks/pre-commit .git/hooks/pre-commit
	@chmod +x .git/hooks/pre-commit
	@echo "Done! Development environment is ready."

# Fast local check (pre-commit). Lints lib+bins only — test code is still
# COMPILED (and any error caught) by `test`, just not clippy-linted here. The
# full clippy --all-targets pass lives in `check-ci` / `lint-full`.
check: fmt-check lint check-chars build test build-examples

# Full check mirroring CI: clippy over all targets incl. tests + examples.
check-ci: fmt-check lint-full check-chars build test build-examples

check-chars:
	apdev-rs check-chars src/

fmt-check:
	cargo fmt --all -- --check

# Fast lint: lib + bins only. Skips re-type-checking all 122 test binaries
# (which `test` compiles anyway) — the single biggest make-check speedup.
lint:
	cargo clippy --lib --bins --all-features -- -D warnings

# Full lint: every target (lib, bins, tests, examples). Slower; use in CI.
lint-full:
	cargo clippy --all-targets --all-features -- -D warnings

build:
	cargo build --all-features

# All integration tests compile into ONE binary (tests/it.rs, autotests=false).
# nextest runs each test in its own PROCESS, restoring the isolation that the
# old 122-binaries-per-file layout gave for free (env-var / global-state tests
# need it). nextest does not run doctests, so those run separately.
test:
	@command -v cargo-nextest >/dev/null 2>&1 || { echo "ERROR: cargo-nextest required (run 'make setup' or 'brew install cargo-nextest')"; exit 1; }
	cargo nextest run --all-features
	cargo test --doc --all-features

# Plain cargo test — runs the consolidated `it` binary single-process. Some
# env-var/global-state tests assume per-test isolation and will fail here; use
# `make test` (nextest) for a green run. Kept for parity debugging.
test-fast:
	cargo test --all-features -- --test-threads=1

build-examples:
	cargo build --examples

fmt:
	cargo fmt --all

clean:
	cargo clean
