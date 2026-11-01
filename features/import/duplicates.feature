Feature: Duplicate albums
  When an album is already in the library, the user decides to keep both,
  skip the new album, or replace the old one.

  Background:
    Given MusicBrainz has the release "Night Lines" by "Mara Vale" from 2021 with the id "new-release":
      | track | title              | artist    | recording   | release track   |
      | 2     | Song (feat. Guest) | Mara Vale | recording-1 | release-track-1 |
    And the incoming song "02 Song.flac" that muzik knows as "Song" by "Mara Vale" on "Night Lines" from 2021
    And the song comes from the source "video-123"
    And the incoming folder has a cover image

  Scenario: A dry run writes nothing, and a duplicate needs a decision
    Given dry run is on
    When muzik plans the import of the songs
    And muzik imports the album as it is and keeps duplicates
    Then the import has 1 destination
    And the imported file does not exist
    And the library has the same albums as before
    When the plan marks album 1 in the library as a duplicate
    And muzik imports the album as it is
    Then the import asks for a duplicate decision
    When muzik imports the album as it is and skips duplicates
    Then the import skipped 1 album

  Scenario: An album whose files are already in the library is skipped
    Given the beets config has the default path "$albumartist/$album/$title"
    When muzik plans the import of the songs
    And muzik imports the album as it is and keeps duplicates
    Given a copy of the song in the folder "again"
    When muzik plans the import of the copy
    And the plan lists no duplicates
    And muzik imports the album as it is
    Then the import fails because the album is already in the library
    When muzik imports the album as it is and skips duplicates
    Then the import found 1 album already in the library
    And the import skipped 1 album
    And the import has no destinations
    And the library has the same albums as before
    And the copy is still in its folder

  Scenario: Replace removes the duplicate album the user selected
    Given the library has the album "Night Lines" by "Mara Vale" with the MusicBrainz id "new-release"
    And that album has a song whose file is missing
    When muzik plans the import of the songs
    And the plan keeps only that album as a duplicate
    Then the planned album has 1 duplicate
    Given dry run is on
    When muzik imports the album with the best match and replaces duplicates
    Given dry run is off
    And the missing song points at the planned destination
    When muzik imports the album with the best match and replaces duplicates
    Then the import added 1 album
    And the imported file exists
    And no old file failed to clean up
    And the new album is in the library
    And the new album has 1 song
    And the song of the new album points at the imported file

  Scenario: Replace keeps an album from a release the user did not select
    Given the library has the album "Night Lines" by "Mara Vale" with the MusicBrainz id "other-release"
    And muzik copies files into the library
    When muzik plans the import of the songs
    Then the planned album lists that album as a duplicate
    When muzik imports the album with the best match and replaces duplicates
    Then that album is still in the library

  Scenario: Replace can reuse the destination of the old album
    Given muzik copies files into the library
    And the import uses the default path "$albumartist/$album/$track $title"
    When muzik plans the import of the songs
    And muzik imports the album as it is and keeps duplicates
    Then the imported file exists
    When muzik plans the import of the songs
    Then the planned album lists the first imported album as a duplicate
    When muzik imports the album as it is and replaces duplicates
    Then the imported file is where the first import put it
    And the imported file exists
    And the first imported album is no longer in the library
    And the new album has 1 song

  Scenario: A failed replace restores the old album and its file
    Given muzik copies files into the library
    When muzik plans the import of the songs
    And muzik imports the album as it is and keeps duplicates
    And muzik plans the import of the songs
    And the incoming song is deleted
    And muzik imports the album as it is and replaces duplicates
    Then the import fails
    And the file from the first import is unchanged
    And the first imported album is still in the library
