# Makefile — invokes helper functions from ./ci/ci_helper.sh for common tasks

SHELL := /bin/bash
CI_SCRIPT := ./ci/ci_helper.sh

.PHONY: setup-environment prepare icons message-groups appendix-d-samples repeating-group-samples regen-readme build build-release build-windows scan coverage sonar release clean help

setup-environment:
	@bash -lc 'source $(CI_SCRIPT) && cmd_setup_environment'

prepare:
	@bash -lc 'source $(CI_SCRIPT) && cmd_setup_environment && ensure_build_metadata && download_fix_specs'
	@cargo run --quiet --bin generate_sensitive_tags >/dev/null

icons:
	@bash -lc './ci/generate_icons.sh'

message-groups:
	@python3 ./ci/generate_message_groups.py

appendix-d-samples:
	@python3 ./ci/generate_appendix_d_samples.py

repeating-group-samples:
	@python3 ./ci/generate_repeating_group_samples.py

regen-readme:
	@python3 ./ci/regen_readme.py

build: prepare
	@bash -lc 'source $(CI_SCRIPT) && ensure_build_metadata && cargo fmt --all && cargo build --workspace'

build-release: prepare
	@bash -lc 'source $(CI_SCRIPT) && ensure_build_metadata && cargo fmt --all && cargo build --workspace --release'

build-windows: prepare
	@bash -lc '\
		source $(CI_SCRIPT) && \
		ensure_build_metadata && \
		ensure_rustup_target x86_64-pc-windows-gnu && \
		ensure_windows_cross_tools && \
		cargo fmt --all && \
		RC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-windres \
		CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
		cargo_with_rustup build --release --workspace --target x86_64-pc-windows-gnu \
	'

scan: prepare
	@bash -lc '\
		source $(CI_SCRIPT) && \
		ensure_build_metadata && \
		cargo fmt --all --check && \
		cargo clippy --workspace --all-targets -- -D warnings && \
		{ \
			if command -v yamllint >/dev/null 2>&1; then \
				yamllint .github/workflows; \
			else \
				echo "yamllint not installed; skipping YAML lint"; \
			fi; \
		} && \
		mkdir -p target/coverage && \
		if command -v cargo-audit >/dev/null 2>&1; then \
			echo "Running cargo-audit (text output)"; \
			if [ -d "$${HOME}/.cargo/advisory-db" ]; then \
				cargo audit --no-fetch || true; \
			else \
				cargo audit || true; \
			fi; \
			echo "Running cargo-audit (SARIF) → target/coverage/rustsec.sarif"; \
			if [ -d "$${HOME}/.cargo/advisory-db" ]; then \
				cargo audit --no-fetch --format sarif > target/coverage/rustsec.sarif || true; \
			else \
				cargo audit --format sarif > target/coverage/rustsec.sarif || true; \
			fi; \
		else \
			echo "cargo-audit not installed; skipping security scan"; \
		fi \
	'

coverage: build
	@bash -lc '\
		source $(CI_SCRIPT) && \
		ensure_llvm_tools_env && \
		ensure_build_metadata && \
		mkdir -p target/coverage && \
		cargo llvm-cov clean --workspace >/dev/null 2>&1 || true; \
		cargo llvm-cov \
		  --package fixdecoder \
		  --package pcap2fix \
		  --cobertura \
		  --ignore-filename-regex "src/fix/sensitive.rs|src/bin/generate_sensitive_tags.rs" \
		  --output-path target/coverage/coverage.xml && \
		python3 ci/normalise_cobertura.py target/coverage/coverage.xml \
	'

sonar:
	@bash -lc '\
		source $(CI_SCRIPT) && \
		if [[ -z "$$(echo "$(MAKECMDGOALS)" | grep -E "(^| )scan( |$$)|(^| )coverage( |$$)")" ]]; then \
			$(MAKE) scan coverage; \
		fi; \
		ensure_sonar_token && \
		ensure_sonar_scanner && \
		sonar-scanner -Dsonar.sarifReportPaths=target/coverage/rustsec.sarif \
	'

release:
	@py=$$(command -v python3 || command -v python || true); \
	if [ -z "$$py" ]; then \
		echo "python3 (or python) is required for release bumping." >&2; \
		exit 1; \
	fi; \
	ver=$$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/'); \
	if [ -z "$$ver" ]; then echo "Could not read version from Cargo.toml" >&2; exit 1; fi; \
	next="$$ver"; \
	while git rev-parse "v$${next}" >/dev/null 2>&1; do \
		next=$$($$py ci/next_patch.py "$${next}"); \
	done; \
	if ! git diff --quiet || ! git diff --cached --quiet; then \
		echo "Working tree is not clean; commit or stash changes before tagging." >&2; \
		exit 1; \
	fi; \
	cleanup() { \
		rc=$$?; \
		if [ $$rc -ne 0 ]; then \
			echo "Release failed; restoring Cargo.toml/Cargo.lock" >&2; \
			git restore --staged Cargo.toml Cargo.lock >/dev/null 2>&1 || true; \
			git restore Cargo.toml Cargo.lock >/dev/null 2>&1 || true; \
		fi; \
		exit $$rc; \
	}; \
	trap 'cleanup' EXIT; \
	$$py ci/bump_version.py "$$ver" "$$next" || exit 1; \
	echo "Bumped version: $$ver -> $$next"; \
	if [ -f Cargo.lock ]; then git add Cargo.toml Cargo.lock; else git add Cargo.toml; fi; \
	git commit -m "chore(release): v$$next"; \
	git tag -a "v$$next" -m "Release v$$next"; \
	git push origin HEAD; \
	git push origin "v$$next"; \
	echo "Created and pushed tag v$$next"; \
	trap - EXIT

clean:
	@cargo clean

help:
	@echo "Available targets:"
	@echo "  setup-environment  → ensure toolchain + coverage tools"
	@echo "  prepare            → setup + build metadata + download FIX specs + regenerate generators"
	@echo "  icons              → regenerate Windows/macOS icon assets from resources/icons/marvin.png"
	@echo "  message-groups     → refresh the explicit MsgType bucket table from official FIX pages"
	@echo "  appendix-d-samples → regenerate the Appendix D sample FIX corpus from the official order-state matrices"
	@echo "  repeating-group-samples → regenerate the checked-in repeating-group FIX examples"
	@echo "  regen-readme       → regenerate README Build It and command-line option output examples"
	@echo "  build              → fmt + cargo build (debug)"
	@echo "  build-release      → fmt + cargo build --release"
	@echo "  build-windows      → fmt + rustup cargo build --release --target x86_64-pc-windows-gnu"
	@echo "  scan               → fmt --check + clippy (+ cargo-audit when available)"
	@echo "  coverage           → cargo llvm-cov --cobertura"
	@echo "  sonar              → sonar-scanner (requires coverage.xml)"
	@echo "  release            → bump patch version, commit, and tag v<version>"
	@echo "  clean              → cargo clean"
	@echo "  help               → this help text"
