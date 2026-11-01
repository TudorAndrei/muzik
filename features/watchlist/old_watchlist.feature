Feature: Old watchlist file
  Earlier versions of muzik kept the watchlist in a JSON file. muzik moves
  that file into the database once, keeps a backup, and keeps the saved state
  of each item. muzik does not move a file that it cannot read.

  Scenario: An old watchlist file moves into the database with its item state
    Given an old watchlist file from version 1
    When the watchlist is loaded
    Then the loaded watchlist is version 3
    And the loaded watchlist has the stages:
      | source | item | stage   | status      |
      | PL1    | 1    | quality | not_started |
      | PL1    | 1    | parse   | complete    |
      | PL1    | 1    | split   | skipped     |
    And item 1 of "PL1" in the loaded watchlist has the download path "/music/song.flac"
    And the loaded watchlist has the processed videos "abcdefghijk" for "PL1"
    And the old watchlist file is replaced by its backup

  Scenario: Saving the moved watchlist again changes nothing
    Given an old watchlist file from version 1
    When the watchlist is loaded
    And the loaded watchlist is saved again
    Then the saved watchlist did not change

  Scenario: An old watchlist file that muzik cannot read stays in place
    Given an old watchlist file with the content:
      """
      {"version":99,"playlists":[]}
      """
    When the source "https://youtube.com/playlist?list=PL_NEW" is added
    Then the source is rejected
    And the old watchlist file still has its content
