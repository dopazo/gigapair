#!/bin/sh
# Runs before the package is removed (deb: prerm, rpm: preun).
#
# On removal only, not on upgrades: stops Gigapair and deletes the autostart
# entry that each user may have created (~/.config/systemd/user/gigapaird.service),
# which would otherwise point at a program that no longer exists. It never
# touches contacts, keys or received files. It must never fail, or the package
# could not be removed.

case "$1" in
  remove|purge|0) ;;   # deb: remove/purge; rpm: 0 = last copy being erased
  *) exit 0 ;;         # upgrade, deconfigure, ...
esac

pkill -x gigapair-app 2>/dev/null || true
pkill -x gigapaird 2>/dev/null || true

getent passwd 2>/dev/null | while IFS=: read -r user _ uid _ _ home _; do
  [ "$uid" -ge 1000 ] 2>/dev/null || continue
  [ "$uid" -lt 65534 ] || continue
  dir="$home/.config/systemd/user"
  [ -f "$dir/gigapaird.service" ] || continue
  rm -f "$dir/gigapaird.service" "$dir"/*.wants/gigapaird.service 2>/dev/null || true
  # Let that user's running systemd forget the unit.
  if [ -d "/run/user/$uid" ] && command -v runuser >/dev/null 2>&1; then
    runuser -u "$user" -- env "XDG_RUNTIME_DIR=/run/user/$uid" \
      systemctl --user daemon-reload 2>/dev/null || true
  fi
done

exit 0
