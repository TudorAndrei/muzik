Feature: Writing files to the device
  muzik writes each file to the device in full or not at all, and it tells
  the user about files whose sync record it could not save.

  Scenario: A copied file is complete and keeps the source permissions
    Given a source file with the content "complete audio"
    When muzik copies the source to a device file with a 240 character name
    Then the device file has the content "complete audio"
    And the device file has the same permissions as the source

  Scenario: A failed copy keeps the earlier device file
    Given a source file with the content "complete audio"
    And muzik copied the source to a device file with a 240 character name
    And the source file is removed
    When muzik copies the source again
    Then the copy fails
    And the device file has the content "complete audio"
    And the device folder has only the device file

  Scenario: Transfers whose record muzik cannot save are counted
    Given an "echo-mini" sync target
    And the library has these tracks:
      | Artist/good.mp3  |
      | Artist/other.mp3 |
    And muzik cannot save sync records
    When muzik prepares the sync with delete off
    And muzik applies the sync
    Then the sync reports 2 transfers and 0 failures
    And 2 transfers are not recorded
