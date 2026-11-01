Feature: Sync planning
  Before muzik writes to the device, it plans where each library file goes,
  which files are already current, and which files it must skip.

  Scenario: Destinations keep the library folders with names that the device accepts
    Given an "echo-mini" sync target
    And the library has these tracks:
      | Artist/Album: Live?/01 Song.flac |
      | Artist/Album: Live?/02 Song.opus |
    And a track "elsewhere.mp3" outside the library
    And the library has the cover "Artist/Album: Live?/cover.jpg"
    When muzik plans the sync
    Then the planned destinations on the device are:
      | Artist/Album_ Live_/01 Song.flac |
      | Artist/Album_ Live_/02 Song.mp3  |
      | Artist/Album_ Live_/cover.jpg    |
    And the track "elsewhere.mp3" is outside the library
    And "Artist/Album_ Live_/02 Song.mp3" needs 2500 bytes

  Scenario: Files that are already on the device are not copied again
    Given an "echo-mini" sync target
    And the library has these tracks:
      | Artist/Album/01 Song.flac |
      | Artist/Album/02 Song.opus |
    And the device has the files that need no conversion
    When muzik plans the sync
    Then the plan has 1 current file and 1 pending file

  Scenario: Only one track goes to each file name on the device
    Given an "mp3" sync target
    And the library has these tracks:
      | Album/01 Song.flac  |
      | Album/01 Song.opus  |
      | Album/01 song.mp3   |
      | Album/02 Other.flac |
    When muzik plans the sync
    Then the planned library tracks are:
      | Album/01 Song.flac  |
      | Album/02 Other.flac |
    And the skipped duplicates are:
      | Album/01 Song.opus |
      | Album/01 song.mp3  |
