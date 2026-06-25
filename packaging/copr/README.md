# Publishing Satur8 to COPR

COPR is Fedora's hosted build service: you point it at this repository, it
builds the RPM for each Fedora release you pick, and users install with a couple
of `dnf` commands. It is the Fedora equivalent of the Arch AUR.

The COPR project is live at https://copr.fedorainfracloud.org/coprs/ntrpydev/satur8/.
The setup notes below are kept for future maintainers or for recreating the
project under another namespace.

## One-time project setup

1. Log in at https://copr.fedorainfracloud.org with a Fedora account.
2. Create a new project named `satur8`.
3. Enable the chroots to build for. Start with the current Fedora x86_64
   releases, for example `fedora-43-x86_64` and `fedora-44-x86_64`.
4. Add a package using the **SCM** source type:
   - Clone URL: `https://github.com/NtrpyDev/satur8`
   - Committish: `main` for the first smoke build, then the release tag,
     e.g. `v0.2.0`
   - Build method: **make srpm** (uses `.copr/Makefile` in this repo)
   - Spec file: `packaging/satur8.spec`
5. Optionally enable auto-rebuild on new tags via a GitHub webhook so a tagged
   release triggers a COPR build automatically.

## What the build does

`.copr/Makefile` regenerates the source archive from the git tree with
`packaging/source-tarball.sh` and builds the source RPM from
`packaging/satur8.spec`. COPR then builds the binary RPM in a clean Fedora
chroot. The spec builds the Rust workspace and the C++ KWin effect, the same
components the Arch package installs.

COPR build chroots have network access, so Cargo fetches crates during the
build. A submission to official Fedora (Koji) would instead require the crates
to be vendored, because Koji builds offline.

## Installing once the project is live

```sh
sudo dnf copr enable ntrpydev/satur8
sudo dnf install satur8
```

## Status

Live and building for Fedora 43 and 44. Treat new Fedora release targets like a
fresh validation pass: the COPR build confirms the `BuildRequires` list and the
Fedora paths in a real Fedora chroot.
