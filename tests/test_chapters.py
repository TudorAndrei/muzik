from pathlib import Path

from muzik.core.chapters import find_chapters, parse_chapters, parse_cue


def test_parse_chapters_reads_editable_text() -> None:
    chapters = parse_chapters(
        """00:00 First
03:12 Second
01:02:03 Third
not a chapter
"""
    )

    assert [
        (chapter.index, chapter.title, chapter.start, chapter.end)
        for chapter in chapters
    ] == [
        (1, "First", 0, 192),
        (2, "Second", 192, 3723),
        (3, "Third", 3723, None),
    ]


def test_parse_cue_reads_track_titles_and_index_times(tmp_path: Path) -> None:
    cue = tmp_path / "Album.cue"
    cue.write_text(
        """
REM GENRE Rock
PERFORMER "Artist"
TITLE "Album"
FILE "Album.flac" WAVE
  TRACK 01 AUDIO
    TITLE "One"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Two"
    INDEX 01 03:12:37
  TRACK 03 AUDIO
    TITLE "Three"
    INDEX 01 07:30:38
""",
        encoding="utf-8",
    )

    chapters = parse_cue(cue)

    assert [
        (chapter.index, chapter.title, chapter.start, chapter.end)
        for chapter in chapters
    ] == [
        (1, "One", 0, 192),
        (2, "Two", 192, 451),
        (3, "Three", 451, None),
    ]


def test_find_chapters_uses_single_cue_in_album_folder(tmp_path: Path) -> None:
    audio = tmp_path / "Album.flac"
    audio.write_bytes(b"")
    (tmp_path / "Album.cue").write_text(
        """
FILE "Album.flac" WAVE
  TRACK 01 AUDIO
    TITLE "One"
    INDEX 01 00:00:00
""",
        encoding="utf-8",
    )

    chapters = find_chapters(audio)

    assert len(chapters) == 1
    assert chapters[0].title == "One"
