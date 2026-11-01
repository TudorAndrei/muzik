Feature: Importing tagged albums
  muzik imports albums the way beets does: the same paths, the same library
  fields, and the MusicBrainz ids of the matched release.

  Scenario: A tagged FLAC file imported as it is gets the beets path and fields
    Given MusicBrainz has the release "Night Lines" by "Mara Vale" from 2021 with the id "new-release":
      | track | title              | artist    | recording   | release track   |
      | 2     | Song (feat. Guest) | Mara Vale | recording-1 | release-track-1 |
    And the tagged incoming song "02 Tide & Stone.flac"
    When muzik plans the import of the songs
    And muzik imports the album as it is
    Then the imported file is at "Mara Vale/Night Lines/02 Tide & Stone.flac" in the library folder
    And the imported song has these fields:
      | comp          | 0     |
      | month         | 4     |
      | day           | 7     |
      | original_year | 2019  |
      | rg_track_gain | -5.25 |
    And the imported album has these fields:
      | comp | 0 |
    And the imported file has these tags:
      | comp | 1 |

  Scenario: An album matched to a recorded MusicBrainz release is imported
    Given MusicBrainz has the recorded release "Dummy" by "Portishead"
    And an incoming song for each track of the release, which muzik knows on the album "Dummy" from 1994
    When muzik plans the import of the incoming folder
    Then the plan has 1 album
    And the planned album has 1 candidate
    And the best candidate pairs 11 songs with tracks
    When muzik imports the album with the best match
    Then the import added 11 songs
    And every imported file is in "Portishead/Dummy" in the library folder
    And the imported song has these fields:
      | mb_albumid        | 76df3287-6cda-33eb-8e9a-044b5e15ffdd |
      | mb_releasetrackid | 17f6c0af-4e48-3810-9d13-17ac5de492df |
    And the imported file has these tags:
      | mb_releasetrackid | 17f6c0af-4e48-3810-9d13-17ac5de492df |
