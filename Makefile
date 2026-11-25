PROJECT ?= $(CURDIR)
# The dev build keeps its own data, apart from the installed app's
# ~/.codeany, and its agent service listens on PORT.
DEV_HOME ?= $(HOME)/.codeany-dev
PORT ?= 18022

.PHONY: dev dev-first-run run bundle release release-package release-publish test

# Dev preview that rebuilds and restarts on every change. PROJECT=none
# launches without a folder, as Finder and the Dock do.
dev:
	@CODEANY_HOME="$(DEV_HOME)" CODEANY_AGL_PORT=$(PORT) scripts/dev.sh "$(PROJECT)"

# The dev preview as a fresh install sees it: empty data (reset on each
# run), opened from the Dock without a folder.
dev-first-run:
	@rm -rf target/first-run-home
	@CODEANY_HOME="$(CURDIR)/target/first-run-home" CODEANY_AGL_PORT=$(PORT) scripts/dev.sh none

# Build and open the dev app once.
run: bundle
	@open -n target/codeany.app --args "$(PROJECT)"

bundle:
	@scripts/bundle.sh

release:
	@scripts/bundle.sh release

release-package:
	@bash scripts/release-mac.sh package

release-publish:
	@bash scripts/release-mac.sh publish

test:
	cargo test
