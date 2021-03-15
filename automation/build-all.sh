#!/bin/bash

set -e

subcrates=()
for a in $(nix eval --raw '(builtins.toString (builtins.attrNames (import ../default.nix)))')
do
    subcrates+=("${a}")
done

args=()
jobs=()
for a in "${subcrates[@]}"
do
    args+=("-A" "${a}")
done

nix-build ../default.nix -j10 --keep-going "${args[@]}" || true

for a in "${subcrates[@]}"
do
    if nix-build ../default.nix "-A" "${a}" >/dev/null 2>/dev/null
    then
        echo "OK   ${a}"
    else
        echo "FAIL ${a}"
    fi
done



(
    cd ../appliance

    nix-build ./default.nix -A startScriptQemu

    if nix-build ./default.nix -A startScriptQemu >/dev/null 2>/dev/null
    then
        echo "OK   appliance disk image"
    else
        echo "FAIL appliance disk image"
    fi
)