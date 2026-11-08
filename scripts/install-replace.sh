#!/bin/sh
# install -D [-m mode] (-t dir file... | file dest), but each file is installed
# under a new temporary name, then renamed into place. Docker Desktop's file
# sharing sometimes refuses to set the mode of a file just recreated at the path
# of one the emulator has open over 9p, as install does. Runs in the stubs
# builder container (see the Makefile).
set -eu

mode=755
dir=
while getopts Dm:t: opt; do
    case $opt in
        D) ;;
        m) mode=$OPTARG ;;
        t) dir=$OPTARG ;;
        *) exit 2 ;;
    esac
done
shift $((OPTIND - 1))

put() {
    mkdir -p "$(dirname "$2")"
    tmp=$(dirname "$2")/.$(basename "$2").$$
    install -m "$mode" "$1" "$tmp"
    mv -f "$tmp" "$2"
}

if [ -n "$dir" ]; then
    for file; do
        put "$file" "$dir/$(basename "$file")"
    done
else
    put "$1" "$2"
fi
