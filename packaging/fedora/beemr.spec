# Fedora package for beemr. Crate dependencies are bundled from the release's
# vendor tarball (Windows-only crates removed) and declared through
# %%cargo_vendor_manifest.

Name:           beemr
Version:        0.2.1
Release:        1%{?dist}
Summary:        Peer-to-peer file and message sharing

SourceLicense:  MIT
# The project is MIT. Bundled Rust crates contribute additional license terms;
# rebuild and check the output of %%{cargo_license_summary} when updating.
License:        MIT
URL:            https://github.com/osmanahmadxai/beemr
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz
Source1:        %{url}/releases/download/v%{version}/%{name}-%{version}-vendor.tar.xz

BuildRequires:  cargo-rpm-macros >= 26

%global _description %{expand:
beemr sends files and messages straight to another device, with no servers,
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

%check
# Unit tests only: the end-to-end tests start several network services and
# a private DHT, which isn't reliable inside isolated build environments.
%cargo_test -- --lib

%files
%license LICENSE LICENSE.dependencies cargo-vendor.txt
%doc README.md PROTOCOL.md
%{_bindir}/%{name}

%changelog
* Sat Oct 03 2026 Osman Ahmadzai <osmanahmadxai@gmail.com> - 0.2.1-1
- Initial package
