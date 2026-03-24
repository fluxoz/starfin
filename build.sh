#!/usr/bin/env bash

set -eo pipefail

mode="$1"

# DEV_SECRETS="dev.env"
# PRD_SECRETS="prd.env"

if [[ -z "$1" ]]; then
	echo "Building in development mode"
	location=$(pwd)
	cd "$location/frontend"
	# sops exec-env "../$DEV_SECRETS" "trunk build"
	STARFIN_DEV=1 trunk build
	cd "$location"
	# sops exec-env "$DEV_SECRETS" "cargo run"
	cargo run
else 
	echo "Building in production mode"
	location=$(pwd)
	cd "$location/frontend"
	# sops exec-env "../$PRD_SECRETS" "trunk build --release"
	trunk build --release
	cd "$location"
	# sops exec-env "$PRD_SECRETS" "cargo build --release"
	cargo build --release
fi
