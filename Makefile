# Makefile for upl
#
# OS auto-detection. Override PREFIX/DESTDIR for custom install locations,
# e.g. `make install PREFIX=$HOME/.local`.

OS := $(shell uname -s 2>/dev/null || echo Windows)
PREFIX ?= /usr/local
DESTDIR ?=

# Cargo binary. When `make install` is run via sudo, cargo is typically not on
# root's PATH (it lives in the invoking user's ~/.cargo/bin). In that case, run
# cargo as the invoking user so the build succeeds and the install step (which
# runs as root) only copies the already-built binary.
CARGO ?= cargo
ifneq ($(SUDO_USER),)
CARGO := sudo -u $(SUDO_USER) bash -lc 'exec cargo "$$@"' _
endif

.PHONY: build test release clean generate_upl_rfc install uninstall

build:
	$(CARGO) build

test:
	$(CARGO) test

release:
	$(CARGO) build --release

clean:
	$(CARGO) clean
	rm -f upl-spec/*.pdf

doc:
	npx mdpdf upl-spec/upl-1.0-rfc.md \
	  --output upl-spec/upl-1.0-rfc.pdf \
	  --css "body{font-family:'Nimbus Roman Regular','Times New Roman',serif}"

# Install the release binary. Detects Linux, macOS and Windows (MinGW/MSYS/Cygwin).
# On Unix it installs to $(DESTDIR)$(PREFIX)/bin (default /usr/local/bin); on
# Windows to $(PREFIX)\bin. Set PREFIX to change the destination.
install: release
	@case "$(OS)" in \
	  Darwin|Linux) \
	    install -d "$(DESTDIR)$(PREFIX)/bin" && \
	    install -m 0755 target/release/upl "$(DESTDIR)$(PREFIX)/bin/upl" && \
	    echo "Installed upl -> $(DESTDIR)$(PREFIX)/bin/upl"; \
	    ;; \
	  MINGW*|MSYS*|CYGWIN*) \
	    mkdir -p "$(PREFIX)/bin" && \
	    cp -f target/release/upl.exe "$(PREFIX)/bin/upl.exe" && \
	    echo "Installed upl.exe -> $(PREFIX)\\bin\\upl.exe"; \
	    ;; \
	  *) \
	    echo "Unsupported OS: $(OS). Build manually with 'cargo build --release'." >&2; exit 1; \
	    ;; \
	esac

uninstall:
	@case "$(OS)" in \
	  Darwin|Linux) \
	    rm -f "$(DESTDIR)$(PREFIX)/bin/upl"; \
	    echo "Removed $(DESTDIR)$(PREFIX)/bin/upl"; \
	    ;; \
	  MINGW*|MSYS*|CYGWIN*) \
	    rm -f "$(PREFIX)/bin/upl.exe"; \
	    echo "Removed $(PREFIX)\\bin\\upl.exe"; \
	    ;; \
	esac
