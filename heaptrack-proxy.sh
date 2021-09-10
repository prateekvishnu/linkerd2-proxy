#!/bin/sh

set -eu

/usr/bin/heaptrack -o /var/run/heaptrack/proxy /usr/lib/linkerd/linkerd2-proxy.bin
