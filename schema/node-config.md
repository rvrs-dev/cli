# A node's tags and location

What a node says about itself in its configuration, and how a deployment package writes it. A node declares its tags and its location. The web app edits on top of that, and the API works out the tags and the location that count ([the node protocol](node-protocol/README.md) has the rules). The CLI only declares.

Status: the contract and its parser are built (`rvrs-core::node_config`, run by `rvrs node` before it refuses). The node itself is not, so nothing is sent yet. Packaging comes later and writes what this page describes.

## The settings

| Setting | Flag | Environment | File key |
| --- | --- | --- | --- |
| Tags | `--tag key=value`, repeatable | `RVRS_NODE_TAGS`, comma-separated `key=value` | `tags`, an object |
| Location | `--location <lat>,<lon>` | `RVRS_NODE_LOCATION`, `lat,lon` | `location.lat` and `location.lon` |
| Place | `--place <text>` | `RVRS_NODE_PLACE` | `location.place` |
| Precision | `--location-precision <exact\|area\|city>` | `RVRS_NODE_LOCATION_PRECISION` | `location.precision` |

Two more name the file: `--config <path>` and `RVRS_NODE_CONFIG`.

```bash
rvrs node --tag isp=telia --tag line=ftth --location 55.5,8.5 --place "Esbjerg, DK"
RVRS_NODE_TAGS=isp=telia,line=ftth RVRS_NODE_LOCATION=55.5,8.5 rvrs node
```

## The file

The file is `node.json` in the config folder (`~/.config/rvrs`, or `$XDG_CONFIG_HOME/rvrs`, or `config` under `RVRS_HOME`). Every file the CLI keeps there is JSON, the CLI has no other parser, and a script on a small device can write JSON with a here-document. So the node's file is JSON too, in the shape the heartbeat carries:

```json
{
  "tags": { "isp": "telia", "line": "ftth", "region": "dk-south" },
  "location": { "lat": 55.5, "lon": 8.5, "place": "Esbjerg, DK", "precision": "city" }
}
```

- Both keys are optional. `place` and `precision` are optional inside `location`. A key this version doesn't know is ignored.
- A missing `node.json` in the config folder is fine. A file named by `--config` or `RVRS_NODE_CONFIG` has to exist.
- A package that must write outside a home folder names its file with `RVRS_NODE_CONFIG` (`/etc/rvrs/node.json`).

## Which source wins

A flag beats the environment, and the environment beats the file.

- Tags merge key by key. A flag's `isp=telia` replaces the file's `isp=tdc`, and the file's other tags stay.
- A location is taken whole from the first source that gives any part of it. If the environment sets `RVRS_NODE_LOCATION`, the file's place and precision are not used with it. A source with a place or a precision and no coordinates is refused.
- A precision nobody set is `city`.

A tag set in the web app is never undone by the node. The node declares, the web edits on top, and the tags that count are the declaration with the edits applied. A node that restarts or declares again leaves a person's edits where they are.

## What is checked

The same rules apply to every source. Each refusal is one sentence that names the input, and a value from the environment or the file also says where it came from. `rvrs node` exits with 2 and sends nothing.

| Input | Rule | The sentence |
| --- | --- | --- |
| A tag without `=` | A tag is a key and a value | The tag “isp” isn't valid. A tag is a key and a value, as in isp=telia. |
| A key | `^[a-z][a-z0-9._-]{0,62}$` | The tag “Isp=Telia” isn't valid. A key is 1 to 63 characters: lowercase letters, digits, ".", "-" and "_", starting with a letter. |
| A value | `^[a-z0-9][a-z0-9._-]{0,62}$` | The tag “isp=Telia” isn't valid. A value is 1 to 63 characters: lowercase letters, digits, ".", "-" and "_", starting with a letter or a digit. |
| The number of tags | 20 at most, after the sources are merged | A node declares at most 20 tags, and these are 21. |
| A location | `lat,lon` in decimal degrees | The location “55.5” isn't valid. It is a latitude and a longitude in decimal degrees, as in 55.5,8.5. |
| A latitude or a longitude | -90 to 90 and -180 to 180 | The location “95,8.5” isn't valid. A latitude runs from -90 to 90. |
| A precision | `exact`, `area` or `city` | The location precision “block” isn't one rvrs knows. Use exact, area or city. |
| A place | 1 to 100 characters on one line. Blank is none | The place “…” is longer than 100 characters. |
| A place or precision alone | Needs coordinates in the same source | A place or a precision needs coordinates. Add --location <lat>,<lon>, as in 55.5,8.5. |
| A flag without its value | | --tag takes a tag, as in --tag isp=telia. |

An empty environment variable counts as unset. A key that comes twice keeps its last value.

The wire is looser than a person's input. The API accepts any precision word up to 20 characters and treats one it doesn't know as `city`, because the list may grow. A person who types a precision gets the three words.

## Privacy

A location is personal data when the node sits in a home. The API rounds the coordinates to the precision before it stores them: `exact` keeps 4 decimals (about 10 m), `area` keeps 2 (about 1 km) and `city` keeps 1 (about 10 km). The default is `city`. A node may declare more digits than the precision keeps, and the API never keeps them. `round_location` in `rvrs-core` shows what will be stored.

Tags and the place are text that everyone in the workspace reads. Put no address, name or other personal data in them.

## What a package writes

Packaging is not built. When it is, a package writes the settings once and the node reads them at every start.

| Package | Writes |
| --- | --- |
| A container | `RVRS_NODE_TAGS`, `RVRS_NODE_LOCATION`, `RVRS_NODE_PLACE` and `RVRS_NODE_LOCATION_PRECISION` as environment variables of the image's command |
| cloud-init | `/etc/rvrs/node.json` through `write_files`, and `RVRS_NODE_CONFIG=/etc/rvrs/node.json` in the unit's environment |
| OpenWrt | uci options (`tags`, `location`, `place`, `precision` in `/etc/config/rvrs`) that the init script turns into the `RVRS_NODE_*` variables of the procd instance |

A package never writes an enrollment token or a node token into these settings. Those come from [the node protocol](node-protocol/README.md).

## Where it is checked

`cargo test` runs `node_config.rs`, which holds every rule above with the environment handed in as a reader, so no test reads the real one. `rvrs node` calls the same code before it refuses. The node protocol's fixtures carry `tags` and `location` in the enroll and the heartbeat, and a test reads them through the same checks.
