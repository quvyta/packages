#!/usr/bin/env python3
"""Builds crates/qpackages/assets/icons/packages.toml: a Nerd Font glyph for well-known packages.

The table below names each package (or name pattern) and the Nerd Fonts glyph it should show.
The script looks each glyph name up in Nerd Fonts' glyphnames.json and keeps a row only when the
glyph really is in a Nerd Font installed on this machine, so the generated file never names a
code point that draws as a blank box.

Usage:
    curl -fsSLo /tmp/glyphnames.json \
        https://raw.githubusercontent.com/ryanoasis/nerd-fonts/master/glyphnames.json
    scripts/icons.py /tmp/glyphnames.json > crates/qpackages/assets/icons/packages.toml

Keys:
    firefox              an exact package name, or a Flatpak id such as org.mozilla.firefox
    python-*             a prefix: every package whose name starts with "python-"
Exact names win over prefixes; a longer prefix wins over a shorter one. The application strips
"-bin", "-git" and "lib32-" before looking a name up, so those variants need no rows here.
"""

import json
import subprocess
import sys

# package name or pattern -> Nerd Fonts glyph name
ICONS = {
    # Browsers
    "firefox": "dev-firefox",
    "firefox-developer-edition": "dev-firefox",
    "org.mozilla.firefox": "dev-firefox",
    "librewolf": "dev-firefox",
    "chromium": "dev-chrome",
    "google-chrome": "dev-chrome",
    "com.google.Chrome": "dev-chrome",
    "opera": "dev-opera",
    "microsoft-edge-stable": "md-microsoft_edge",
    "torbrowser-launcher": "linux-tor",
    "tor": "linux-tor",
    # Mail and chat
    "thunderbird": "linux-thunderbird",
    "org.mozilla.Thunderbird": "linux-thunderbird",
    "discord": "fa-discord",
    "com.discordapp.Discord": "fa-discord",
    "telegram-desktop": "fa-telegram",
    "org.telegram.desktop": "fa-telegram",
    "signal-desktop": "md-signal",
    "org.signal.Signal": "md-signal",
    "slack-desktop": "fa-slack",
    "com.slack.Slack": "fa-slack",
    "skypeforlinux-bin": "md-skype",
    "teams-for-linux": "md-microsoft_teams",
    "whatsapp-for-linux": "md-whatsapp",
    "mastodon": "md-mastodon",
    # Audio and video
    "vlc": "md-vlc",
    "org.videolan.VLC": "md-vlc",
    "mpv": "linux-mpv",
    "io.mpv.Mpv": "linux-mpv",
    "kodi": "md-kodi",
    "spotify": "fa-spotify",
    "spotify-launcher": "fa-spotify",
    "com.spotify.Client": "fa-spotify",
    "obs-studio": "md-video",
    "com.obsproject.Studio": "md-video",
    "kdenlive": "linux-kdenlive",
    "org.kde.kdenlive": "linux-kdenlive",
    "youtube-music": "md-youtube",
    "freetube": "md-youtube",
    "twitch": "fa-twitch",
    # Graphics
    "gimp": "linux-gimp",
    "org.gimp.GIMP": "linux-gimp",
    "inkscape": "linux-inkscape",
    "org.inkscape.Inkscape": "linux-inkscape",
    "krita": "linux-krita",
    "org.kde.krita": "linux-krita",
    "blender": "dev-blender",
    "org.blender.Blender": "dev-blender",
    # Office and notes
    "libreoffice-fresh": "linux-libreoffice",
    "libreoffice-still": "linux-libreoffice",
    "org.libreoffice.LibreOffice": "linux-libreoffice",
    "obsidian": "custom-obsidian",
    "md.obsidian.Obsidian": "custom-obsidian",
    "notion-app-electron": "dev-notion",
    "calibre": "md-book_open_variant",
    "okular": "md-file_pdf_box",
    "evince": "md-file_pdf_box",
    "zathura": "md-file_pdf_box",
    # Games
    "steam": "fa-steam",
    "com.valvesoftware.Steam": "fa-steam",
    "minecraft-launcher": "md-minecraft",
    "prismlauncher": "md-minecraft",
    "itch-setup-bin": "fa-itch_io",
    "wine": "md-bottle_wine",
    # Development
    "code": "dev-vscode",
    "visual-studio-code": "dev-vscode",
    "com.visualstudio.code": "dev-vscode",
    "vscodium": "dev-vscode",
    "neovim": "custom-neovim",
    "vim": "custom-vim",
    "gvim": "custom-vim",
    "emacs": "custom-emacs",
    "sublime-text-4": "dev-sublime",
    "intellij-idea-community-edition": "dev-intellij",
    "pycharm-community-edition": "dev-pycharm",
    "android-studio": "md-android_studio",
    "git": "dev-git",
    "github-cli": "dev-github",
    "glab": "dev-gitlab",
    "gitea": "linux-gitea",
    "python": "dev-python",
    "python-*": "dev-python",
    "rust": "dev-rust",
    "rustup": "dev-rust",
    "cargo-*": "dev-rust",
    "go": "md-language_go",
    "nodejs": "fa-node_js",
    "npm": "dev-npm",
    "yarn": "dev-yarn",
    "jdk-openjdk": "dev-java",
    "jre-openjdk": "dev-java",
    "kotlin": "custom-kotlin",
    "ruby": "custom-ruby",
    "ruby-*": "custom-ruby",
    "php": "dev-php",
    "lua": "dev-lua",
    "lua-*": "dev-lua",
    "ghc": "dev-haskell",
    "haskell-*": "dev-haskell",
    "julia": "dev-julia",
    "gcc": "dev-gcc",
    "llvm": "dev-llvm",
    "clang": "dev-llvm",
    "cmake": "dev-cmake",
    "docker": "dev-docker",
    "podman": "dev-podman",
    "kubectl": "dev-kubernetes",
    "helm": "dev-helm",
    "ansible": "dev-ansible",
    "terraform": "dev-terraform",
    "vagrant": "dev-vagrant",
    "aws-cli-v2": "dev-aws",
    "azure-cli": "dev-azure",
    "mariadb": "dev-mariadb",
    "mysql": "dev-mysql",
    "postgresql": "dev-database",
    "sqlite": "dev-sqlite",
    "redis": "dev-redis",
    "mongodb-bin": "dev-mongodb",
    "nginx": "dev-nginx",
    "apache": "dev-apache",
    "filezilla": "dev-filezilla",
    "openssh": "md-ssh",
    "tmux": "dev-tmux",
    "zsh": "custom-zsh",
    "fish": "md-fish",
    "bash": "dev-bash",
    # System
    "linux": "linux-tux",
    "linux-lts": "linux-tux",
    "linux-zen": "linux-tux",
    "linux-hardened": "linux-tux",
    "pacman": "linux-archlinux",
    "archlinux-keyring": "linux-archlinux",
    "plasma-desktop": "linux-kde_plasma",
    "plasma-meta": "linux-kde_plasma",
    "gnome-shell": "linux-gnome",
    "xfce4-session": "linux-xfce",
    "hyprland": "linux-hyprland",
    "sway": "linux-sway",
    "i3-wm": "linux-i3",
    "bluez": "fa-bluetooth",
    "networkmanager": "md-wifi",
    "cups": "md-printer",
    "ufw": "md-shield",
    "keepassxc": "md-key",
    "bitwarden": "md-shield_lock",
    "dropbox": "fa-dropbox",
    "transmission-gtk": "md-download",
    "qbittorrent": "md-download",
    "teamviewer": "md-teamviewer",
    # Fonts
    "ttf-*": "md-format_font",
    "otf-*": "md-format_font",
    "noto-fonts*": "md-format_font",
    "adobe-source-*": "md-format_font",
}

# Nerd Font families to check glyphs against, in order; the first one found is used.
FONTS = ["Symbols Nerd Font Mono", "Symbols Nerd Font", "JetBrainsMono Nerd Font Mono"]


def font_file():
    for family in FONTS:
        out = subprocess.run(
            ["fc-match", "--format=%{file}\n%{family}", family], capture_output=True, text=True, check=True
        ).stdout.split("\n")
        if len(out) == 2 and family.split()[0] in out[1]:
            return out[0]
    sys.exit("no Nerd Font found; install ttf-nerd-fonts-symbols-mono")


def charset(path):
    text = subprocess.run(["fc-query", "--format=%{charset}", path], capture_output=True, text=True, check=True).stdout
    covered = set()
    for part in text.split():
        start, _, end = part.partition("-")
        covered.update(range(int(start, 16), int(end or start, 16) + 1))
    return covered


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    glyphs = json.load(open(sys.argv[1], encoding="utf-8"))
    covered = charset(font_file())
    print("# A Nerd Font glyph for well-known packages, as hexadecimal code points.")
    print("# Generated by scripts/icons.py; edit the table there and run it again.")
    print("# `name-*` keys are prefixes. Unlisted packages show their category's icon.")
    print()
    print("[icons]")
    missing = []
    for key in sorted(ICONS, key=str.lower):
        name = ICONS[key]
        entry = glyphs.get(name)
        if entry is None or int(entry["code"], 16) not in covered:
            missing.append(f"{key} -> {name}")
            continue
        print(f'"{key}" = "{entry["code"]}"  # {name}')
    for line in missing:
        print(f"skipped: {line}", file=sys.stderr)


if __name__ == "__main__":
    main()
