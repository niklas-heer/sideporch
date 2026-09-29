# The live demo runs a release image; `fly deploy --build-arg VERSION=main`
# tries the newest build of main first. Fly mounts its volume at /data
# owned by root, and the image has no shell to change that, so the program
# runs as root here, inside Fly's single-purpose virtual machine.
ARG VERSION=0.6.0
FROM ghcr.io/niklas-heer/sideporch:${VERSION}
USER root
