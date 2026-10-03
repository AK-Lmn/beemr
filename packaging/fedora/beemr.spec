# Fedora package for beemr. Crate dependencies are bundled from the release's
# vendor tarball (Windows-only crates removed) and declared through
# %%cargo_vendor_manifest.

Name:           beemr
Version:        0.3.0
Release:        1%{?dist}
Summary:        Peer-to-peer file sharing

SourceLicense:  MIT
# Licenses of the bundled Rust crates, from %%{cargo_license_summary}:
# (MIT OR Apache-2.0) AND Apache-2.0
# (MIT OR Apache-2.0) AND Unicode-3.0
# Apache-2.0
# Apache-2.0 AND ISC
# Apache-2.0 OR ISC OR MIT
# Apache-2.0 OR MIT
# Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT
# BSD-2-Clause OR MIT
# BSD-3-Clause
# ISC
# ISC AND (Apache-2.0 OR ISC)
# ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0)
# MIT
# MIT OR Apache-2.0
# MIT OR Apache-2.0 OR BSD-1-Clause
# MIT OR Apache-2.0 OR LGPL-2.1-or-later
# MIT OR Apache-2.0 OR Zlib
# MIT OR BSD-3-Clause
# MPL-2.0
# Unicode-3.0
# Unlicense OR MIT
# Zlib
# Zlib OR Apache-2.0 OR MIT
License:        %{shrink:
    MIT AND
    Apache-2.0 AND
    BSD-3-Clause AND
    ISC AND
    MPL-2.0 AND
    Unicode-3.0 AND
    Zlib AND
    (Apache-2.0 OR ISC OR MIT) AND
    (Apache-2.0 OR ISC OR MIT-0) AND
    (Apache-2.0 OR MIT) AND
    (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT) AND
    (BSD-2-Clause OR MIT) AND
    (MIT OR Apache-2.0 OR BSD-1-Clause) AND
    (MIT OR Apache-2.0 OR LGPL-2.1-or-later) AND
    (MIT OR Apache-2.0 OR Zlib) AND
    (MIT OR BSD-3-Clause) AND
    (Unlicense OR MIT) AND
    (Zlib OR Apache-2.0 OR MIT)
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
* Sat Oct 03 2026 Osman Ahmadzai <osmanahmadxai@gmail.com> - 0.3.0-1
- Initial package
