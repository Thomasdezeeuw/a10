# Command to run in `dev` target, e.g. `make RUN=check dev`.
RUN ?= test

# NOTE: when using this command you might want to change the `test` target to
# only run a subset of the tests you're actively working on.
dev:
	find src/ tests/ examples/ Makefile Cargo.toml | entr -d -c $(MAKE) $(RUN)

test:
	cargo test

check:
	cargo check --all-targets

lint: clippy
clippy:
	cargo clippy --all-features --workspace -- \
		--deny clippy::all \
		--deny clippy::correctness \
		--deny clippy::style \
		--deny clippy::complexity \
		--deny clippy::perf \
		--deny clippy::pedantic \
		--deny clippy::nursery \
		--deny clippy::cargo \
		--allow clippy::needless-lifetimes \
		--allow clippy::match-bool \
		--allow clippy::use-self \
		--allow clippy::single-match-else \
		--allow clippy::missing-errors-doc \
		--allow clippy::missing-panics-doc

doc_private:
	cargo doc --document-private-items

clean:
	cargo clean

.PHONY: dev test check lint clippy doc_private clean
