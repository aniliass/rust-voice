# Rust Voice

Proximity voice chat for [Horizon](https://playhorizon.dev), the Rust rewrite of Minecraft 26.3. Talk to players on servers running **Plasmo Voice** or **Simple Voice Chat**, including players who use those mods on the regular Java launcher.

**A native Horizon mod**: the launcher compiles it into the game. Native mods are not sandboxed and run with your account's rights, so read the source before you build it.

## What it does

- **Two protocols in one mod.** On joining a server it asks for Plasmo Voice (v2) and Simple Voice Chat (2.6 and 2.7, compatibility 20 and 21) and uses whichever the server answers. On Horizon servers it also talks to Horizon's own voicechat-server (Simple Voice Chat 2.6 over Horizon's mod messages). The Advanced tab shows which one is in use.
- **Clean microphone:** high-pass filter, automatic gain with a soft limiter, a noise gate with pre-roll and hysteresis, and a spectral noise suppressor. *Learn noise* records the room's own hum so it can be removed.
- **3D voices:** stereo panning with head shadow (HRTF-like timing and level differences), quieter and duller with distance (the air takes the highs away), muffled behind walls.
- **Sound goes around corners:** when a wall is between you, the voice comes from the doorway or corridor it travels through (a path search over nearby blocks) instead of straight through the wall.
- **Badges over names:** a microphone icon over each player that fills purple with their voice level; a crossed-out microphone when they muted, crossed-out headphones when their sound is off, a grey "no" sign when they have no voice mod. Dimmed behind walls. The badge sits exactly where the game draws the name and doesn't lag when players run or jump.
- **Notes over a playing disc:** while a server plays a disc or another sound at a point in the world (pv-addon-discs), coloured notes fly up from it in time with the music, like a note block's, and shatter against blocks above it. A small tag over the jukebox shows the track name with a spinning disc and level bars. Drawn by the mod itself, hidden behind walls; both can be turned off in the Overlay tab. A jukebox walled in by blocks sounds muffled.
- **Groups menu for Plasmo Voice:** on servers with the pv-addon-groups add-on, the Groups tab lists the server's groups with their members, joins and leaves them, invites nearby players and creates new ones (name and password typed right in the menu). The add-on itself only has chat commands; the mod sends them and reads the answers, keeping them out of chat.
- **Simple Voice Chat groups:** the same Groups tab on Simple Voice Chat servers — groups and members, joining (with a password), leaving and creating a group of a chosen type: normal, open or isolated. In a normal group two keys work: *Groups* talks to the group, *Local* only to players nearby (meanwhile the mod steps out of the group and comes back; the start of the phrase isn't lost). If you are alone in the group, the mod doesn't leave it.
- **Groups at a glance:** while you talk, the HUD shows a white microphone for nearby voice and a purple microphone when your voice goes to your group (Plasmo Voice groups addon, Simple Voice Chat groups). Players talking in your group get a purple frame and, if you like, their own list in a corner you choose, separate from everyone else (Overlay tab, can be turned off). The Groups volume slider sets their volume.
- **World sounds from Plasmo Voice servers:** music discs (pv-addon-discs) and other sounds the server places in the world play from their jukebox or entity, muffled behind walls and going around corners like voices, with their own *Music and sounds* volume.
- **Plasmo Voice activations:** voice activation, push to talk on up to three keys, per-activation distance, as the server defines them.
- **English, Russian and Ukrainian interface:** follows the language set in the game (Options > Language), or pick one on the Advanced tab.
- **Menu** (key **M**, or `/voice`, `/vm`): devices, per-player volume (scroll with the wheel), activations, overlay, hotkeys (rebind inside the menu) and advanced settings. `/voice mute` and `/voice deaf` toggle the microphone and the voice chat.
- **Player volume with the wheel:** hold the right mouse button, look at a player and scroll — a small purple tag with the percentage appears over their name.

## What it doesn't do (yet)

- Simple Voice Chat **whisper** is not sent.
- Simple Voice Chat location sounds (from server plugins) are played, but not placed in the world.
- On Simple Voice Chat 2.6 other players can't see that your microphone is off (that protocol version doesn't carry it) — only that your sound is off.

## What it touches

- **Network:** one UDP connection to the voice port of the Minecraft server you are playing on (the address and port that server sends). Plasmo Voice audio is AES-CBC encrypted with the key the server sends (exchanged with RSA); Simple Voice Chat packets are AES-128-GCM encrypted with the server's secret. It contacts no other host.
- **Microphone and speakers:** through cpal, only while you are connected to a voice server. The microphone is only sent while an activation is on (voice activation or a held push-to-talk key).
- **Files**, all in `~/.config` (on Windows `%USERPROFILE%\.config`): `horizon-voice-mod.conf` (settings), `horizon-voice-mod.noise` (learned noise profile), `horizon-voice-mod.mute` (microphone switch) and `horizon-voice-mod.log` (log). Nothing else is read or written.

## Building

```sh
sh pack.sh            # build/rust_voice-<version>.hnmod
```

The `.hnmod` holds `horizon-native.json`, the bundle's `.rs` and `.toml` files and `LICENSE`, the same files as horizon-mods' `native.py package`. Unit tests: `cargo test` in `mixins/rust_voice/api`.

Icons are generated by `tools/gen_icons.py` (`src/icons.rs`) and `tools/mod_icon.py` (`assets/icon.png`).

## Not affiliated

Not made, endorsed or supported by the Plasmo Voice or Simple Voice Chat projects. It contains none of their code or assets: the protocols are implemented from scratch in Rust, and the icons are drawn for this project.

## Credits

Developed by AniLiaSS with AI assistance (Claude by Anthropic). The author decides what goes in, tests it in the game and is responsible for the mod.

## License

MIT, see [LICENSE](LICENSE).
