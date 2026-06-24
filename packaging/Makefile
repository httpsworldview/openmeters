# Targets: check tarball deb rpm all checksums clean

REPO_ROOT := $(CURDIR)/..
VERSION := $(shell awk '/^\[package\]/{p=1;next} /^\[/{p=0} p && /^version[[:space:]]*=/{n=split($$0,a,"\""); print a[2]; exit}' $(REPO_ROOT)/Cargo.toml)
$(if $(VERSION),,$(error could not parse version from $(REPO_ROOT)/Cargo.toml))

RELEASE ?= 1
MIN_PIPEWIRE_VERSION ?= 0.3.65
MAX_GLIBC_VERSION ?=

TRIPLE := $(shell rustc -vV 2>/dev/null | awk '/^host:/ {sub(/unknown-/,""); print $$2}')

DIST := $(REPO_ROOT)/dist
STAGE := $(DIST)/openmeters-$(VERSION)-$(TRIPLE)
RELEASE_BIN := $(REPO_ROOT)/target/release/openmeters
override GLIBC_VERSION := $(shell \
	objdump -T $(RELEASE_BIN) 2>/dev/null \
	| awk 'match($$0,/GLIBC_[0-9]+([.][0-9]+)*/) { print substr($$0, RSTART + 6, RLENGTH - 6) }' \
	| sort -Vu \
	| tail -n 1)

TARBALL := $(DIST)/openmeters-$(VERSION)-$(TRIPLE).tar.xz
DEB     := $(DIST)/openmeters_$(VERSION)-$(RELEASE)_amd64.deb
RPM     := $(DIST)/openmeters-$(VERSION)-$(RELEASE).x86_64.rpm

SOURCE_DATE_EPOCH ?= $(shell git -C $(REPO_ROOT) log -1 --pretty=%ct 2>/dev/null)

TAR_REPRO_FLAGS := --owner=0 --group=0 --numeric-owner --sort=name \
                   --mtime=@$(SOURCE_DATE_EPOCH)

export VERSION
export RELEASE
export GLIBC_VERSION
export MIN_PIPEWIRE_VERSION
export SOURCE_DATE_EPOCH

.PHONY: all check nfpm-check tarball deb rpm checksums clean

all: tarball deb rpm checksums

check:
	@test -n "$(TRIPLE)" || { echo "could not determine host triple from rustc" >&2; exit 1; }
	@test "$(TRIPLE)" = "x86_64-linux-gnu" || { echo "packaging supports only x86_64 GNU/Linux; got $(TRIPLE)" >&2; exit 1; }
	@test -n "$(SOURCE_DATE_EPOCH)" || { echo "could not determine SOURCE_DATE_EPOCH from git" >&2; exit 1; }
	@test -x "$(RELEASE_BIN)" || { echo "missing $(RELEASE_BIN); run cargo build --locked --release first" >&2; exit 1; }
	@test -n "$(GLIBC_VERSION)" || { echo "could not determine GLIBC requirement from $(RELEASE_BIN)" >&2; exit 1; }
	@if [ -n "$(MAX_GLIBC_VERSION)" ]; then \
		newest="$$(printf '%s\n%s\n' "$(GLIBC_VERSION)" "$(MAX_GLIBC_VERSION)" | sort -Vu | tail -n 1)"; \
		if [ "$$newest" != "$(MAX_GLIBC_VERSION)" ]; then \
			echo "$(RELEASE_BIN) requires glibc $(GLIBC_VERSION), above allowed $(MAX_GLIBC_VERSION)" >&2; \
			exit 1; \
		fi; \
	fi
	@echo "==> openmeters $(VERSION) $(TRIPLE), glibc >= $(GLIBC_VERSION), PipeWire >= $(MIN_PIPEWIRE_VERSION)"

nfpm-check: check
	@command -v nfpm >/dev/null || { echo "missing nfpm; install https://nfpm.goreleaser.com/" >&2; exit 1; }

$(DIST):
	mkdir -p $(DIST)

# Tarball
tarball: check | $(DIST)
	rm -rf $(STAGE)
	install -Dm0755 $(RELEASE_BIN)                         $(STAGE)/bin/openmeters
	install -Dm0644 $(REPO_ROOT)/misc/openmeters.desktop   $(STAGE)/share/applications/openmeters.desktop
	install -Dm0644 $(REPO_ROOT)/misc/openmeters.svg       $(STAGE)/share/icons/hicolor/scalable/apps/openmeters.svg
	install -Dm0644 $(REPO_ROOT)/LICENSE                   $(STAGE)/LICENSE
	install -Dm0644 $(REPO_ROOT)/packaging/copyright       $(STAGE)/share/doc/openmeters/copyright
	install -Dm0644 $(REPO_ROOT)/LICENSE                   $(STAGE)/share/licenses/openmeters/LICENSE
	install -Dm0644 $(REPO_ROOT)/docs/licenses/iced_widget_pane_grid.md $(STAGE)/share/licenses/openmeters/iced_widget_pane_grid.md
	install -Dm0644 $(REPO_ROOT)/README.md                 $(STAGE)/README.md
	tar $(TAR_REPRO_FLAGS) -cJf $(TARBALL) -C $(DIST) openmeters-$(VERSION)-$(TRIPLE)
	@echo "==> $(TARBALL)"

# nfpm (deb / rpm)
deb: nfpm-check | $(DIST)
	nfpm package --config $(CURDIR)/nfpm.yaml --packager deb --target $(DIST)/
	@echo "==> $(DEB)"

rpm: nfpm-check | $(DIST)
	nfpm package --config $(CURDIR)/nfpm.yaml --packager rpm --target $(DIST)/
	@echo "==> $(RPM)"

# Checksums
checksums: tarball deb rpm
	cd $(DIST) && sha256sum \
	    $(notdir $(TARBALL)) \
	    $(notdir $(DEB)) \
	    $(notdir $(RPM)) > SHA256SUMS
	@echo "==> $(DIST)/SHA256SUMS"

clean:
	rm -rf $(DIST)
