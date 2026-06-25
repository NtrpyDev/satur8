# COPR build entry point.
#
# COPR's "Make SRPM" build method clones this repository and runs:
#
#     make -f .copr/Makefile srpm outdir=<dir>
#
# We generate the source archive the spec expects (the same archive the GitHub
# release carries, produced reproducibly from the git tree), then build the
# source RPM from packaging/satur8.spec. COPR takes the resulting .src.rpm and
# builds the binary RPMs for each enabled Fedora chroot.

srpm:
	dnf -y install rpm-build git tar gzip
	bash packaging/source-tarball.sh
	mkdir -p $(outdir)
	rpmbuild -bs packaging/satur8.spec \
		--define "_sourcedir $(CURDIR)/target/dist" \
		--define "_srcrpmdir $(outdir)"
