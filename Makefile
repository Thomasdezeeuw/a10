# Command to run in `dev` target, e.g. `make RUN=check dev`.
RUN ?= test

# NOTE: when using this command you might want to change the `test` target to
# only run a subset of the tests you're actively working on.
dev:
	find src/ tests/ examples/ Makefile Cargo.toml | entr -d -c $(MAKE) $(RUN)

test:
	cargo test

test_sanitizers:
	$(MAKE) test_sanitizer sanitizer=address
	$(MAKE) test_sanitizer sanitizer=thread

# NOTE: Fails with:
# LeakSanitizer: CHECK failed: lsan_interceptors.cpp:82 "((!lsan_init_is_running)) != (0)" (0x0, 0x0)
#$(MAKE) test_sanitizer sanitizer=leak
# NOTE: Fails because it doesn't understand the kernel is writing into a e.g. a
# read buffer, without an actual system call.
#$(MAKE) test_sanitizer sanitizer=memory

# Run with `make test_sanitizer sanitizer=$sanitizer`, or use `test_sanitizers`.
test_sanitizer:
	RUSTDOCFLAGS=-Zsanitizer=$(sanitizer) RUSTFLAGS=-Zsanitizer=$(sanitizer) \
	cargo test -Zbuild-std --target x86_64-unknown-linux-gnu

check:
	cargo check --all-targets

# TODO: Review and fix:
# * cast-possible-wrap
# * cast-sign-loss
# * cast-possible-truncation
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
		--allow clippy::must-use-candidate \
		--allow clippy::return-self-not-must-use \
		--allow clippy::redundant-pub-crate \
		--allow clippy::len-without-is-empty \
		--allow clippy::single-match-else \
		--allow clippy::missing-errors-doc \
		--allow clippy::missing-panics-doc \
		--allow clippy::cast-possible-wrap \
		--allow clippy::cast-sign-loss \
		--allow clippy::cast-lossless \
		--allow clippy::cast-possible-truncation \

doc_private:
	cargo doc --document-private-items

clean:
	cargo clean

.PHONY: dev test test_sanitizers test_sanitizer check lint clippy doc_private clean
