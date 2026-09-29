# The live demo runs the release image. Fly mounts its volume at /data owned
# by root, and the image has no shell to change that, so the program runs
# as root here, inside Fly's single-purpose virtual machine.
FROM ghcr.io/niklas-heer/sideporch:0.6.0
USER root
