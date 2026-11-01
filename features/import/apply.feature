Feature: Applying an import
  muzik puts the planned songs into the library folder, writes their tags,
  and adds them to the beets library.

  Background:
    Given MusicBrainz has the release "Night Lines" by "Mara Vale" from 2021 with the id "new-release":
      | track | title              | artist    | recording   | release track   |
      | 2     | Song (feat. Guest) | Mara Vale | recording-1 | release-track-1 |
    And the incoming song "02 Song.flac" that muzik knows as "Song" by "Mara Vale" on "Night Lines" from 2021
    And the song comes from the source "video-123"
    And the incoming folder has a cover image

  Scenario: The best match gives the song clean tags, a cover, and its source
    When muzik plans the import of the songs
    Then the planned album has 1 candidate
    When muzik imports the album with the best match and keeps duplicates
    Then the import added 1 song
    And the incoming songs are still there
    And the imported file is named "02 Song.flac"
    And the imported file has these tags:
      | title      | Song                  |
      | artist     | Mara Vale feat. Guest |
      | mb_albumid | new-release           |
    And the imported file has a front cover
    And the imported song has the source "video-123"
    And the imported song has these fields:
      | title | Song |
    And the imported album has these fields:
      | mb_albumid | new-release |

  Scenario: A singleton goes to the singleton path and gets no album
    Given the import uses the singleton path "Singles/$artist/$title"
    When muzik plans the import of the songs as singletons
    Then the plan has 1 album
    And the planned album is a singleton
    And the planned album has no candidates
    When muzik imports the album as it is
    Then the import added no albums
    And the imported file is at "Singles/Mara Vale/Song.flac" in the library folder
    And the imported file exists
    And the imported song belongs to no album

  Scenario: A move removes the incoming song after the library write
    Given muzik moves files into the library
    When muzik plans the import of the songs as singletons
    And muzik imports the album as it is
    Then no incoming song failed to clean up
    And the incoming song is gone
    And the imported file exists
    And the imported song is in the library

  Scenario: A beets replace rule changes the destination
    Given the beets replace rule changes "Song" to "Tune"
    And dry run is on
    When muzik plans the import of the songs
    Then the planned album is an album
    When muzik imports the album with the best match and keeps duplicates
    Then the imported file is named "02 Tune.flac"
