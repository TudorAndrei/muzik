Feature: Saving the watchlist safely
  The desktop app, the command line, and the job runner can change the
  watchlist at the same time. A checked write saves only when nobody changed
  the watchlist after it was read, so no change is lost.

  Background:
    Given the watched playlist "PL1" has the video "song" titled "Song"

  Scenario: A checked write keeps a source edit made at the same time
    Given a check has read the watchlist
    When another editor renames the source "PL1" to "Edited"
    And the check saves the title "Checked" for item 1 of "PL1"
    Then the checked write is refused as a conflict
    And the saved source "PL1" has the title "Edited"
    And item 1 of "PL1" in the saved watchlist has the title "Song"
    When the check reads the watchlist again
    And the check saves the title "Checked" for item 1 of "PL1"
    Then the checked write is saved
    And the saved source "PL1" has the title "Edited"
    And item 1 of "PL1" in the saved watchlist has the title "Checked"
    And the saved revision is 1 more than the revision that the check read

  Scenario: A checked write keeps a stage change made at the same time
    Given a check has read the watchlist
    When a runner completes the download of item 1 of "PL1"
    And the check saves what it read
    Then the checked write is refused as a conflict
    And the saved watchlist has the stages:
      | source | item | stage    | status   |
      | PL1    | 1    | download | complete |

  Scenario: A checked write of an unchanged watchlist keeps the revision
    Given a check has read the watchlist
    When the check saves what it read
    Then the checked write is saved
    And the saved watchlist did not change

  Scenario: A checked write that fails changes nothing
    Given a check has read the watchlist
    And the database refuses new watchlist items
    When the check renames the source "PL1" to "Renamed" and adds the item "New"
    Then the checked write fails
    And the saved source "PL1" has no title
    And the saved source "PL1" has 1 item
    And the saved revision is the revision that the check read

  Scenario: Parallel updates keep every change
    When 8 workers each mark a different video of "PL1" as processed at the same time
    Then the saved watchlist has 8 processed videos for "PL1"
