PROJECT ?= $(CURDIR)

.PHONY: dev run bundle release test

# Dev preview that rebuilds and restarts on every change.
dev:
	@scripts/dev.sh "$(PROJECT)"

# Build and open the dev app once.
run: bundle
	@open -n target/codeany.app --args "$(PROJECT)"

bundle:
	@scripts/bundle.sh

release:
	@scripts/bundle.sh release

test:
	cargo test
