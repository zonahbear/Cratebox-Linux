# build environment for this workspace (not needed on a normal Linux machine with libasound2-dev)
export PKG_CONFIG_PATH=/home/claude/.local/alsa-shim/pkgconfig
export CARGO_HTTP_PROXY=$HTTPS_PROXY
export no_proxy=localhost NO_PROXY=localhost
