Feature: Songs without an album
  A folder with one song becomes a song without an album, so players do not
  show an album with one song. The song keeps its own tags and its cover. The
  song stays on an album when it is a single release or when its album is
  already in the library.

  Background:
    Given MusicBrainz has the release "Night Lines" by "Mara Vale" from 2021 with the id "new-release":
      | track | title              | artist    | recording   | release track   |
      | 2     | Song (feat. Guest) | Mara Vale | recording-1 | release-track-1 |
    And the incoming song "02 Song.flac" that muzik knows as "Song" by "Mara Vale" on "Night Lines" from 2021
    And the incoming folder has a cover image

  Scenario: A song alone in its folder becomes a singleton with an embedded cover
    Given autotag is off
    When muzik plans the import of the songs
    Then the planned album is a singleton
    When muzik imports the album as it is
    Then the import added no albums
    And the imported file is at "Non-Album/Mara Vale/Song.flac" in the library folder
    And the imported song belongs to no album
    And the imported file has a front cover
    And the library folder "Non-Album/Mara Vale" has no cover file
    And the imported song has these attributes:
      | muzik_original_album | Night Lines |
    And the imported file has these tags:
      | title  | Song      |
      | artist | Mara Vale |
    And the imported file has none of these tags:
      | album       |
      | albumartist |
      | track       |

  Scenario: A song that matches a single release stays an album
    When muzik plans the import of the songs
    Then the planned album has 1 candidate
    And the planned album is an album

  Scenario: A song alone in its folder joins its album in the library
    Given autotag is off
    And the library has the album "Night Lines" by "Mara Vale" with the MusicBrainz id "old-release"
    And that album has the song "Other Song"
    When muzik plans the import of the songs
    Then the planned album is an album
    When muzik imports the album as it is
    Then that album has 2 songs
