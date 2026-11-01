Feature: Import plan
  Before muzik imports anything, it groups the files into albums and looks
  for matching releases on MusicBrainz. The plan shows what the import will do.

  Scenario: Songs in an album folder become one album matched to a release
    Given MusicBrainz has the release "Night Lines" by "Mara Vale" from 2021 with the id "release-1":
      | track | title        | artist    | recording                            | release track   |
      | 2     | Tide & Stone | Mara Vale | 11111111-1111-4111-8111-111111111111 | release-track-1 |
    And the tagged incoming song "02 Tide & Stone.flac"
    And the song comes from the source "video-123"
    When muzik plans the import of the incoming folder and its song
    Then the plan has 1 album
    And the planned album comes from the incoming folder
    And the planned album has 1 song
    And the planned song is the incoming song
    And the planned song comes from the source "video-123"
    And the planned song has the length of its audio file
    And the planned album has 1 candidate
    And the best candidate is the release "release-1"
    And the best candidate pairs the song with the first track
    And the planned album has 1 duplicate

  Scenario: An album is planned as it is when MusicBrainz fails
    Given MusicBrainz is not reachable
    And the incoming song "Night Lines.flac" that muzik knows as "Song" by "Mara Vale" on "Night Lines"
    When muzik plans the import of the songs
    Then the plan has 1 album
    And the planned album has no candidates
    And the planned song has the artist "Mara Vale"

  Scenario: With autotag off, muzik plans the import without MusicBrainz
    Given MusicBrainz is not reachable
    And autotag is off
    And the blank incoming song "Song.flac"
    When muzik plans the import of the songs
    Then MusicBrainz was not asked
    And the plan has 1 album
    And the planned album has no candidates

  Scenario: Cancelling during the MusicBrainz search stops the plan
    Given an empty beets library
    And the import is cancelled during the MusicBrainz search
    And the incoming song "song.flac" that muzik knows as "Song" by "Mara Vale" on "Night Lines"
    When muzik plans the import of the songs
    Then the plan is cancelled
