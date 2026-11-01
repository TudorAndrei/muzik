Feature: Removing old files from the device
  With delete on, a sync removes the files on the device that are not in the
  selection. muzik keeps them when it cannot read all the selected tracks.

  Background:
    Given an "echo-mini" sync target
    And the device has the old file "Old/old.mp3" of 7 bytes

  Scenario: A track that muzik cannot read stops the delete
    Given the library has these tracks:
      | Artist/good.mp3   |
      | Artist/broken.mp3 |
    And muzik cannot read the track "broken.mp3"
    When muzik prepares the sync with delete on
    Then the delete is blocked
    And no old files are set for removal
    And the delete frees 0 bytes

  Scenario: Old files are set for removal when muzik can read all tracks
    Given the library has these tracks:
      | Artist/good.mp3 |
    When muzik prepares the sync with delete on
    Then the delete is allowed
    And the old file "Old/old.mp3" is set for removal
    And the delete frees 7 bytes

  Scenario: The sync removes old files and copies the new tracks
    Given the library has these tracks:
      | Artist/good.mp3  |
      | Artist/other.mp3 |
    When muzik prepares the sync with delete on
    And muzik applies the sync
    Then the device does not have "Old/old.mp3"
    And the device does not have "Old"
    And the 2 planned files are on the device
    And the sync reports 2 transfers and 0 failures
    And each transfer is reported and recorded

  Scenario: An old file that is already gone does not stop the sync
    Given the library has these tracks:
      | Artist/good.mp3  |
      | Artist/other.mp3 |
    When muzik prepares the sync with delete on
    And someone removes "Old/old.mp3" from the device
    And muzik applies the sync
    Then the 2 planned files are on the device
    And the sync reports 2 transfers and 0 failures

  Scenario: The sync changes nothing when the device is unplugged
    Given the library has these tracks:
      | Artist/good.mp3 |
    When muzik prepares the sync with delete on
    And the device is unplugged
    And muzik applies the sync
    Then the sync fails because the device is missing
    And the unplugged device still has "Old/old.mp3"
    And the device folder does not exist
