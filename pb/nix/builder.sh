#!/bin/bash
set -ex
unset PATH
unset PKG_CONFIG_PATH

for p in $buildInputs $nativeBuildInputs; do
  if [ -d "$p/bin" ]
  then
    PATH="${p}/bin${PATH:+:}${PATH}"
  fi
  if [ -d "${p}/lib/pkgconfig" ]
  then
    PKG_CONFIG_PATH="${p}/lib/pkgconfig${PKG_CONFIG_PATH:+:}${PKG_CONFIG_PATH}"
  fi
done

export PATH
export PKG_CONFIG_PATH

env
mkdir "$out"
ls -l "${src}"
cp -r "${src}"/* "${out}"
(cd "${src}" && "${BUILDER}")