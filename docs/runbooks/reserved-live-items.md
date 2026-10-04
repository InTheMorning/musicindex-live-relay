# Reserved Live Items Runbook

This runbook is for the operator of the relay. It covers the reserved live
items of ADR 0001. `README.md` is the full API reference.

The commands use these values. Change them for your host.

- The relay listens on `http://127.0.0.1:8018`.
- The unit is `musicindex-live-relay.service`.
- The state file is `/var/lib/musicindex-live-relay/reserved-items.sqlite3`.
- The admin header file is `/etc/musicindex-live-relay/admin.header`.

## Before You Start

A reserved item needs the admin token. Without `ADMIN_TOKEN`, the reserve,
list and delete routes answer `404 reserved_items_disabled`, and the relay
opens no state file.

Keep each secret out of command arguments and logs. A command argument is
visible to other users in the process list.

1. Make the directory for the admin files:
   `sudo install -d -m 0700 /etc/musicindex-live-relay`
2. Make an empty environment file with mode `0600`:
   `sudo install -m 0600 /dev/null /etc/musicindex-live-relay/admin.env`
3. Put one line in the environment file: `ADMIN_TOKEN=<admin token>`. Use a
   token with 16 bytes or more.
4. Make an empty header file with mode `0600`:
   `sudo install -m 0600 /dev/null /etc/musicindex-live-relay/admin.header`
5. Put one line in the header file: `Authorization: Bearer <admin token>`.
6. Open a drop-in for the unit: `sudo systemctl edit musicindex-live-relay`
7. Add these two lines to the drop-in:

   ```ini
   [Service]
   EnvironmentFile=/etc/musicindex-live-relay/admin.env
   ```

8. Restart the relay: `sudo systemctl restart musicindex-live-relay`
9. Read the startup line: `journalctl -u musicindex-live-relay -n 20`
10. Make sure that the log has `restored reserved live items` with the state
    file path and a count.

## Reserve An Item And Store The Broadcaster Token

The relay returns the broadcaster token one time only. If you lose the token
before you store it, the item is not usable. Delete it and reserve a new item.

1. Set a file mask that keeps new files private: `umask 077`
2. Reserve the item, and write the response to a private file:

   ```sh
   sudo curl -sS -X POST \
     -H @/etc/musicindex-live-relay/admin.header \
     -H 'Content-Type: application/json' \
     -d '{"label":"weekly show"}' \
     http://127.0.0.1:8018/v1/liveitems/reserved > reserve.json
   ```

3. Make sure that `reserve.json` holds `event_id` and `broadcaster_token`. A
   `409` means that another reserved item has the label.
4. Write the token to the token file of the publisher target:

   ```sh
   jq -r .broadcaster_token reserve.json \
     > ~/.config/musicindex-live-publisher/mixxx/tokens/default.token
   ```

5. Make sure that the token file has mode `0600` and its directory has mode
   `0700`.
6. Write the `event_id` from `reserve.json` into the publisher target.
7. Put the event URI into the `podcast:liveValue` block of the RSS feed. See
   `README.md`, section "RSS `podcast:liveValue`".
8. Delete `reserve.json`: `shred -u reserve.json`

## Back Up The State File

The state file holds the identifier, the label, the creation time and the
token hash of each reserved item. It holds no payload and no token.

**A lost state file kills every reserved item at the next restart.** Each
stored broadcaster token becomes invalid. Each RSS feed that names a reserved
item points to an event that does not exist. You must reserve each item
again, give each publisher a new token, and change each feed.

The relay writes the file only for a reserve and a delete. Make a backup after
each reserve and each delete.

1. Stop the relay: `sudo systemctl stop musicindex-live-relay`
2. Copy the file and keep its mode:

   ```sh
   sudo cp -p /var/lib/musicindex-live-relay/reserved-items.sqlite3 \
     /root/reserved-items-$(date -u +%Y%m%dT%H%M%SZ).sqlite3
   ```

3. Start the relay: `sudo systemctl start musicindex-live-relay`
4. Keep the backup with mode `0600`. It holds token hashes.

A backup is a copy at one time. Know these two results before you restore one:

- An item that you reserved after the backup is not in it. That item dies.
- An item that you deleted after the backup is in it. That item comes back,
  and its old broadcaster token is valid again. Delete it again after the
  restore.

## Rotate The Admin Token

The relay keeps the admin token in memory only. A rotation does not change the
state file, and each broadcaster token stays valid.

A rotation restarts the relay. Each ephemeral item dies, and each reserved item
has no snapshot until its next publish. Rotate when no show is on air.

1. Make a new token with 16 bytes or more: `openssl rand -base64 32`
2. Put the new token in `/etc/musicindex-live-relay/admin.env`.
3. Put the new token in `/etc/musicindex-live-relay/admin.header`.
4. Restart the relay: `sudo systemctl restart musicindex-live-relay`
5. List the reserved items with the new token:

   ```sh
   sudo curl -sS -H @/etc/musicindex-live-relay/admin.header \
     http://127.0.0.1:8018/v1/liveitems/reserved
   ```

6. Make sure that the list answers `200`. A `403` means that the two files
   hold different tokens.
7. Give the new token to each tool that uses the admin routes.

## What A Restart Looks Like

A restart keeps each reserved item and its broadcaster token. The state file
holds no payload, so a restored item has no snapshot. This is correct. It is
not a defect. A restored snapshot can send listener payments to a track that
stopped hours before.

Until the next publish, a restored item gives these answers:

- `GET /v1/liveitems/{event_id}/remoteValue` gives `200` with `{}`.
- Socket.IO sends `{}` at connect.
- `GET /v1/liveitems/{event_id}/metadata` gives `404` with the error code
  `metadata_not_found`. The item exists. Only the error code
  `event_not_found` means that an item does not exist.
- A keepalive gives `409` with the error code `lease_expired`. The
  broadcaster must publish. A keepalive alone does not bring the item back
  on air.
- The sequence number starts again at zero. An SSE client that keeps an old
  `Last-Event-ID` gets no replay.
- The list shows no `last_publish_at` for the item.

The first publish brings the item back on air with the same identifier.

## Delete A Reserved Item And Tell Listeners

**A delete is permanent.** The relay removes the row from the state file. A
restart does not bring the item back, and its broadcaster token becomes
invalid.

1. List the reserved items:

   ```sh
   sudo curl -sS -H @/etc/musicindex-live-relay/admin.header \
     http://127.0.0.1:8018/v1/liveitems/reserved
   ```

2. Find the `event_id` of the item. Use its `label`.
3. Stop the publisher target that uses the item.
4. Delete the item:

   ```sh
   sudo curl -sS -X DELETE -w '%{http_code}\n' \
     -H @/etc/musicindex-live-relay/admin.header \
     http://127.0.0.1:8018/v1/liveitems/reserved/<event_id>
   ```

5. Make sure that the answer is `204`. A `404 event_not_found` means that no
   reserved item has the identifier.
6. In each RSS feed, remove the `podcast:liveValue` block of the item. As an
   alternative, put the URI of a new item in the block.
7. Remove the token file of the item from the publisher configuration.
8. Make a backup of the state file. See "Back Up The State File".

The relay tells the connected listener apps. Each SSE stream ends. Each
Socket.IO client gets `{}` and is disconnected. Each route of the identifier
then answers `404 event_not_found`. A listener app that reads the RSS feed
again finds the change.

## Recover From A Corrupt State File

The relay does not start with a corrupt or unreadable state file. It does not
change, delete or make again the file. Five failed starts in 300 seconds stop
the restart loop, and systemd marks the unit `failed`.

1. Read the unit state: `systemctl status musicindex-live-relay`
2. Read the error: `journalctl -u musicindex-live-relay -n 20`
3. Find the line `relay stopped with an error`. It names the state file and
   the cause.
4. If the cause is a schema version, do not change the file. Install the relay
   build that wrote the file, and stop here.
5. Copy the damaged file to a safe place. Keep it for analysis:

   ```sh
   sudo cp -p /var/lib/musicindex-live-relay/reserved-items.sqlite3 \
     /root/reserved-items-corrupt-$(date -u +%Y%m%dT%H%M%SZ).sqlite3
   ```

6. If you have a backup, copy it into place:

   ```sh
   sudo install -m 0600 /root/reserved-items-<time>.sqlite3 \
     /var/lib/musicindex-live-relay/reserved-items.sqlite3
   ```

7. If you have no backup, remove the damaged file. The relay makes a new empty
   file at the next start:
   `sudo rm /var/lib/musicindex-live-relay/reserved-items.sqlite3`
8. Clear the failed state: `sudo systemctl reset-failed musicindex-live-relay`
9. Start the relay: `sudo systemctl start musicindex-live-relay`
10. Read the startup line, and make sure that the restored count is correct.
11. If you used a backup, delete each item that you deleted after the backup.
12. If you had no backup, reserve each item again. Give each publisher its new
    token, and change each RSS feed.
