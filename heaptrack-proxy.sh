#!/bin/sh

set -eu

/usr/bin/heaptrack /usr/lib/linkerd/linkerd2-proxy.bin
