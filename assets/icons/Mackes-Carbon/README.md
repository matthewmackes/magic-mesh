# Mackes-Carbon — IBM Carbon icon theme for GNOME & XFCE

A [freedesktop.org Icon Theme Specification][spec] theme built from the
[IBM Carbon Design System][carbon] icon set. Every glyph is a scalable SVG
carrying `fill="currentColor"`, so the icons recolor with the desktop's GTK/Qt
style context (light or dark) like any modern symbolic theme.

- **4570 SVGs** across the 8 standard contexts — `actions`, `apps`,
  `categories`, `devices`, `emblems`, `mimetypes`, `places`, `status`.
- Names follow the [Icon Naming Specification][naming] (`folder`, `user-home`,
  `audio-volume-high`, `text-x-generic`, …), each with a `-symbolic` companion,
  so GNOME Shell and XFCE find icons by their standard names out of the box.
- The `apps` context includes generated aliases for common desktop launchers:
  browsers, mail and chat clients, office suites, editors, developer tools,
  media apps, graphics apps, games, virtualization tools, and system utilities.
- Falls back through `hicolor` then `Adwaita` (`Inherits=` in `index.theme`) for
  any name this set does not define.

## Install

```sh
./install.sh             # current user  -> ~/.local/share/icons/Mackes-Carbon
sudo ./install.sh --system   # all users -> /usr/share/icons/Mackes-Carbon
./install.sh --uninstall     # remove (add --system if installed there)
```

The installer copies the theme and rebuilds the GTK icon cache. Then pick it:

| Desktop | GUI | CLI |
|---|---|---|
| GNOME | Tweaks → Appearance → Icons | `gsettings set org.gnome.desktop.interface icon-theme 'Mackes-Carbon'` |
| XFCE  | Settings → Appearance → Icons | `xfconf-query -c xsettings -p /Net/IconThemeName -s 'Mackes-Carbon'` |

### Manual install

Drop the `Mackes-Carbon/` directory into any XDG icon path
(`~/.local/share/icons/` or `/usr/share/icons/`) and run
`gtk-update-icon-cache -f -t <path>/Mackes-Carbon`.

## License

The icon geometry is IBM Carbon, **Apache License 2.0** — see `LICENSE` and
`NOTICE`. The only modification to upstream SVGs is the injection of
`fill="currentColor"`; files are renamed/reorganized into the freedesktop
directory layout, with common application aliases copied byte-for-byte from the
matching Carbon glyph.

## Maintenance

Regenerate the common application aliases after changing the alias map:

```sh
python3 tools/generate-common-app-aliases.py
```

The generator is idempotent and refuses to overwrite an existing SVG whose
geometry differs from the selected Carbon source.

[spec]: https://specifications.freedesktop.org/icon-theme-spec/latest/
[naming]: https://specifications.freedesktop.org/icon-naming-spec/latest/
[carbon]: https://carbondesignsystem.com/elements/icons/library/
