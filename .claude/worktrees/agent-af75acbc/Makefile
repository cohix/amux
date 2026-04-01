BINARY     := amux
INSTALL_PATH ?= /usr/local/bin

.PHONY: all build install test clean release

all: build

build:
	cargo build --release

install: build
	install -m 755 target/release/$(BINARY) $(INSTALL_PATH)/$(BINARY)

test:
	cargo test --quiet

clean:
	cargo clean

release:
	@if [ -z "$(VERSION)" ]; then \
		echo "Usage: make release VERSION=vx.y.z"; \
		exit 1; \
	fi
	@bash scripts/release.sh "$(VERSION)"
