Feature: Library sync
  muzik refreshes library entries from MusicBrainz. It updates the beets
  library and the tags in the audio files.

  Scenario: Sync updates an album and the tags of its file
    Given MusicBrainz has the release "New Album Title" by "New Album Artist" from 2022 with the id "release-1":
      | track | title                  | artist       | recording   | release track   |
      | 1     | Other Song             | Other Artist | recording-1 | release-track-1 |
      | 2     | New Song (feat. Guest) | New Artist   | recording-1 | release-track-2 |
    And the library has the album "SyncTest" with the MusicBrainz id "release-1"
    And that album has the song "Old Song" as track 1 of the recording "recording-1" and the release track "release-track-2"
    When muzik syncs "album:SyncTest" and writes tags
    Then the sync updated 1 album and 1 song
    And the synced song has these fields:
      | title  | New Song               |
      | artist | New Artist feat. Guest |
      | track  | 2                      |
    And the synced file has these tags:
      | title  | New Song               |
      | artist | New Artist feat. Guest |

  Scenario: Sync updates a singleton by its recording
    Given MusicBrainz has the recording "recording-singleton" called "Fresh Song (feat. Guest)" by "Solo Artist"
    And the library has the singleton "OldSong" of the recording "recording-singleton"
    When muzik syncs "title:OldSong" and writes tags
    Then the sync updated 1 singleton and 1 song
    And the synced song has these fields:
      | title  | Fresh Song              |
      | artist | Solo Artist feat. Guest |
    And the synced file has these tags:
      | title | Fresh Song |
