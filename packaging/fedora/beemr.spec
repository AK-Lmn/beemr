# Fedora package for beemr. Crate dependencies are bundled from the release's
# vendor tarball (Windows-only crates removed) and declared through
# %%cargo_vendor_manifest.

Name:           beemr
Version:        0.4.0
Release:        1%{?dist}
Summary:        Peer-to-peer file sharing

SourceLicense:  MIT
# Licenses of the bundled Rust crates, from %%{cargo_license_summary}:
# (MIT OR Apache-2.0) AND Apache-2.0
# (MIT OR Apache-2.0) AND Unicode-3.0
# 0BSD OR MIT OR Apache-2.0
# Apache-2.0
# Apache-2.0 AND ISC
# Apache-2.0 OR ISC OR MIT
# Apache-2.0 OR MIT
# Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT
# BSD-2-Clause
# BSD-2-Clause OR Apache-2.0 OR MIT
# BSD-2-Clause OR MIT
# BSD-3-Clause
# BSD-3-Clause OR MIT OR Apache-2.0
# BSL-1.0
# CC0-1.0
# ISC
# ISC AND (Apache-2.0 OR ISC)
# ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0)
# LGPL-3.0-or-later OR MPL-2.0
# MIT
# MIT OR Apache-2.0 OR Zlib
# MPL-2.0
# Unicode-3.0
# Unlicense
# Unlicense OR MIT
# Zlib
# SQLite (bundled by libsqlite3-sys for Tor's directory cache): blessing
License:        %{shrink:
    MIT AND
    Apache-2.0 AND
    BSD-2-Clause AND
    BSD-3-Clause AND
    BSL-1.0 AND
    CC0-1.0 AND
    ISC AND
    MPL-2.0 AND
    Unicode-3.0 AND
    Unlicense AND
    Zlib AND
    blessing AND
    (0BSD OR MIT OR Apache-2.0) AND
    (Apache-2.0 OR ISC OR MIT) AND
    (Apache-2.0 OR ISC OR MIT-0) AND
    (Apache-2.0 OR ISC) AND
    (Apache-2.0 OR MIT) AND
    (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT) AND
    (BSD-2-Clause OR Apache-2.0 OR MIT) AND
    (BSD-2-Clause OR MIT) AND
    (BSD-3-Clause OR MIT OR Apache-2.0) AND
    (LGPL-3.0-or-later OR MPL-2.0) AND
    (MIT OR Apache-2.0 OR Zlib) AND
    (Unlicense OR MIT)
}
# LICENSE.dependencies contains the full per-crate license breakdown.
URL:            https://github.com/osmanahmadxai/beemr
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz
Source1:        %{url}/releases/download/v%{version}/%{name}-%{version}-vendor.tar.xz

BuildRequires:  cargo-rpm-macros >= 26

%global _description %{expand:
beemr sends files and folders straight to another device, with no servers,
accounts or setup. It connects directly when it can, punches through home and
mobile NATs when it can't, and falls back to relaying through other beemr
devices. Devices find each other through the BitTorrent Mainline DHT, and all
traffic is end-to-end encrypted.}

%description %{_description}

%prep
%autosetup -n %{name}-%{version} -p1 -a1
%cargo_prep -v vendor

%build
%cargo_build
%{cargo_license_summary}
%{cargo_license} > LICENSE.dependencies
%cargo_vendor_manifest

%install
install -Dpm 0755 target/rpm/%{name} %{buildroot}%{_bindir}/%{name}
install -Dpm 0644 docs/%{name}.1 %{buildroot}%{_mandir}/man1/%{name}.1

%check
# Unit tests only: the end-to-end tests start several network services and
# a private DHT, which isn't reliable inside isolated build environments.
%cargo_test -- --lib

%files
%license LICENSE LICENSE.dependencies cargo-vendor.txt
%doc README.md PROTOCOL.md
%{_bindir}/%{name}
%{_mandir}/man1/%{name}.1*

%changelog
* Sat Oct 03 2026 Osman Ahmadzai <osmanahmadxai@gmail.com> - 0.4.0-1
- Update to 0.4.0

* Sat Oct 03 2026 Osman Ahmadzai <osmanahmadxai@gmail.com> - 0.3.0-1
- Initial package
