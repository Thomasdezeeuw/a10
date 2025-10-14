# Command to run in `dev` target, e.g. `make RUN=check dev`.
RUN ?= test

# NOTE: when using this command you might want to change the `test` target to
# only run a subset of the tests you're actively working on.
dev:
	find src/ tests/ examples/ Makefile Cargo.toml | entr -d -c $(MAKE) $(RUN)

test:
	cargo test -- --quiet
	cargo test --all-features -- --quiet

test_sanitizers:
	$(MAKE) test_sanitizer sanitizer=address
	$(MAKE) test_sanitizer sanitizer=thread
	$(MAKE) test_sanitizer sanitizer=memory
	$(MAKE) test_sanitizer sanitizer=leak

# Run with `make test_sanitizer sanitizer=$sanitizer`, or use `test_sanitizers`.
test_sanitizer:
	RUSTDOCFLAGS=-Zsanitizer=$(sanitizer) RUSTFLAGS=-Zsanitizer=$(sanitizer) \
	cargo test --features nightly -Zbuild-std --target x86_64-unknown-linux-gnu

check:
	cargo check --all-targets

# Disabled lints:
# * `doc-markdown`: has some annoying false positives.
# * `equatable-if-let`: strongly disagree with this lint.
# * `missing-const-for-fn`: has false positives.
# * `missing-errors-doc`, `missing-panics-doc`: not worth it.
# * `must-use-candidate`: too many bad suggestions.
# * `needless-lifetimes`: lifetimes are additional docs.
# * `option-if-let-else`: don't want to introduce more closures.
# * `redundant-pub-crate`: useless lint.
# * `single-match-else`: prefer match statements over if statements.
# * `use-self`: strongly disagree.
# TODO: resolve the lints after the empty line.
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
		--allow clippy::doc-markdown \
		--allow clippy::equatable-if-let \
		--allow clippy::missing-const-for-fn \
		--allow clippy::missing-errors-doc \
		--allow clippy::missing-panics-doc \
		--allow clippy::must-use-candidate \
		--allow clippy::needless-lifetimes \
		--allow clippy::new-without-default \
		--allow clippy::option-if-let-else \
		--allow clippy::redundant-pub-crate \
		--allow clippy::single-match-else \
		--allow clippy::use-self \
		\
		--allow clippy::cast-possible-truncation \
		--allow clippy::elidable-lifetime-names \

doc:
	cargo doc --all-features

doc_private:
	cargo doc --document-private-items

clean:
	cargo clean

.PHONY: dev test test_sanitizers test_sanitizer check lint clippy doc doc_private clean
