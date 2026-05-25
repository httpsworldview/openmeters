# Targets: tarball deb rpm all checksums clean

REPO_ROOT := $(CURDIR)/..
VERSION := $(shell awk '/^\[package\]/{p=1;next} /^\[/{p=0} p && /^version[[:space:]]*=/{n=split($$0,a,"\""); print a[2]; exit}' $(REPO_ROOT)/Cargo.toml)
$(if $(VERSION),,$(error could not parse version from $(REPO_ROOT)/Cargo.toml))

RELEASE ?= 1

TRIPLE := $(shell rustc -vV 2>/dev/null | awk '/^host:/ {sub(/unknown-/,""); print $$2}')
$(if $(TRIPLE),,$(error could not determine host triple from rustc))

DIST := $(REPO_ROOT)/dist
STAGE := $(DIST)/openmeters-$(VERSION)-$(TRIPLE)
RELEASE_BIN := $(REPO_ROOT)/target/release/openmeters

TARBALL := $(DIST)/openmeters-$(VERSION)-$(TRIPLE).tar.xz
DEB     := $(DIST)/openmeters_$(VERSION)-$(RELEASE)_amd64.deb
RPM     := $(DIST)/openmeters-$(VERSION)-$(RELEASE).x86_64.rpm

SOURCE_DATE_EPOCH ?= $(shell git -C $(REPO_ROOT) log -1 --pretty=%ct 2>/dev/null)
$(if $(SOURCE_DATE_EPOCH),,$(error could not determine SOURCE_DATE_EPOCH from git))

TAR_REPRO_FLAGS := --owner=0 --group=0 --numeric-owner --sort=name \
                   --mtime=@$(SOURCE_DATE_EPOCH)

export VERSION
export RELEASE
export SOURCE_DATE_EPOCH

.PHONY: all tarball deb rpm checksums clean

all: tarball deb rpm checksums

$(DIST):
	mkdir -p $(DIST)

# Tarball
tarball: | $(DIST)
	rm -rf $(STAGE)
	mkdir -p $(STAGE)/bin
	mkdir -p $(STAGE)/share/applications
	mkdir -p $(STAGE)/share/icons/hicolor/scalable/apps
	mkdir -p $(STAGE)/share/doc/openmeters
	mkdir -p $(STAGE)/share/licenses/openmeters
	install -m 0755 $(RELEASE_BIN)                         $(STAGE)/bin/openmeters
	install -m 0644 $(REPO_ROOT)/misc/openmeters.desktop   $(STAGE)/share/applications/openmeters.desktop
	install -m 0644 $(REPO_ROOT)/misc/openmeters.svg       $(STAGE)/share/icons/hicolor/scalable/apps/openmeters.svg
	install -m 0644 $(REPO_ROOT)/LICENSE                   $(STAGE)/LICENSE
	install -m 0644 $(REPO_ROOT)/packaging/copyright       $(STAGE)/share/doc/openmeters/copyright
	install -m 0644 $(REPO_ROOT)/LICENSE                   $(STAGE)/share/licenses/openmeters/LICENSE
	install -m 0644 $(REPO_ROOT)/docs/licenses/iced_widget_pane_grid.md $(STAGE)/share/licenses/openmeters/iced_widget_pane_grid.md
	install -m 0644 $(REPO_ROOT)/README.md                 $(STAGE)/README.md
	tar $(TAR_REPRO_FLAGS) -cJf $(TARBALL) -C $(DIST) openmeters-$(VERSION)-$(TRIPLE)
	@echo "==> $(TARBALL)"

# nfpm (deb / rpm)
deb: | $(DIST)
	nfpm package --config $(CURDIR)/nfpm.yaml --packager deb --target $(DIST)/
	@echo "==> $(DEB)"

rpm: | $(DIST)
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
