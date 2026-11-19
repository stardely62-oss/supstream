#!/usr/bin/env python3
"""Имитация yt-dlp для тестов: отвечает данными по ключевому слову в запросе
и «скачивает» файл с фиксированным содержимым."""
import json
import sys

args = sys.argv[1:]
target = args[-1]
query = target.removeprefix("ytsearch1:")

KNOWN = {
    "clean": ("clean1", "Good Band", "Clean Song", 30.0),
    "dirty": ("dirty1", "Bad Band", "Dirty Song", 30.0),
    "slur": ("slur1", "Worse Band", "Slur Song", 30.0),
    "nolyrics": ("nol1", "Unknown", "Obscure Track", 30.0),
    "long": ("long1", "Prog", "Epic", 9999.0),
}

if "fail" in query:
    print("ERROR: [youtube] fail: Video unavailable", file=sys.stderr)
    sys.exit(1)

if "-J" in args:
    key = next((k for k in KNOWN if query.startswith(k)), None)
    if key:
        vid, artist, track, dur = KNOWN[key]
    else:
        vid, artist, track, dur = ("q_" + "".join(c for c in query if c.isalnum()), "Some", query, 60.0)
    entry = {
        "id": vid,
        "title": f"{artist} - {track} (Official Video)",
        "webpage_url": f"https://www.youtube.com/watch?v={vid}",
        "duration": dur,
        "uploader": artist,
        "extractor_key": "Youtube",
        "thumbnail": None,
    }
    if query.startswith("http"):
        print(json.dumps(entry))
    else:
        print(json.dumps({"_type": "playlist", "entries": [entry]}))
    sys.exit(0)

template = args[args.index("-o") + 1]
path = template.replace("%(ext)s", "webm")
import os
import shutil

real = os.environ.get("FAKE_AUDIO_FILE")
if real:
    shutil.copyfile(real, path)  # настоящий звук — для проверки плеера в браузере
else:
    with open(path, "wb") as f:
        f.write(b"FAKEAUDIO" * 100)
print(path)
