#!/usr/bin/env bash

DIR=$(dirname $0)
DID_SOMETHING=""

# Check if config directory exists
CONF_DIR="$HOME/.config/rfm"
if [[ -e "$CONF_DIR" ]]; then
  echo "Found config directory \"$CONF_DIR\""
else
  echo "Creating config directory \"$CONF_DIR\""
  mkdir -p "$CONF_DIR"
fi

# Check if key-config exists
KEY_CONF="$CONF_DIR/keys.toml"
if [[ -e "$KEY_CONF" ]]; then
  echo "Found key-config..."
else
  echo "Copying default key-config to \"$KEY_CONF\""
  cp $DIR/examples/keys.toml $KEY_CONF
  DID_SOMETHING="Copied key-config"
fi

# Check if opening-config exists
OPEN_CONF="$CONF_DIR/open.toml"
if [[ -e "$OPEN_CONF" ]]; then
  echo "Found opening-config..."
else
  echo "Copying default opening-config to \"$OPEN_CONF\""
  cp $DIR/examples/open.toml $OPEN_CONF
  DID_SOMETHING="Copied opening-config"
fi

if [[ -z "$DID_SOMETHING" ]]; then
  echo "Nothing to do."
else
  echo "Done."
fi
