BINARY     := aspec
INSTALL_PATH ?= /usr/local/bin

.PHONY: all build install test clean

all: build

build:
	cargo build --release

install: build
	install -m 755 target/release/$(BINARY) $(INSTALL_PATH)/$(BINARY)

test:
	cargo test

clean:
	cargo clean
