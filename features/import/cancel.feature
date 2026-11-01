Feature: Cancelling an import
  The user can cancel an import at any time. A cancelled import leaves the
  incoming songs, the library folder, and the beets library as they were.

  Background:
    Given MusicBrainz has the release "Night Lines" by "Mara Vale" from 2021 with the id "new-release":
      | track | title              | artist    | recording   | release track   |
      | 2     | Song (feat. Guest) | Mara Vale | recording-1 | release-track-1 |
    And the incoming song "02 Song.flac" that muzik knows as "Song" by "Mara Vale" on "Night Lines" from 2021
    And the song comes from the source "video-123"
    And the incoming folder has a cover image

  Scenario: Cancelling before the import starts changes nothing
    When muzik plans the import of the songs as singletons
    And muzik imports the album as it is, but the import is cancelled before it starts
    Then the import is cancelled
    And the incoming songs are still there
    And the library has the same songs and albums as before
    And the library folder has no files

  Scenario: Cancelling after the first file is moved puts the files back
    Given the incoming song "03 Other.flac" that muzik knows as "Other" by "Mara Vale" on "Night Lines"
    And autotag is off
    And muzik moves files into the library
    When muzik plans the import of the songs
    And muzik imports the album as it is and keeps duplicates, but the import is cancelled after the first file is placed
    Then the import is cancelled
    And the incoming songs are still there
    And the library has the same songs and albums as before
    And the library folder has no files
