Feature: Local check of the watchlist
  When the watchlist page opens, muzik shows the saved state at once and then
  checks the local files in the background. The result of the check is saved
  and shown only while no job runs and no newer edit or check started.

  Background:
    Given the watched playlist "PL1" has the video "abcdefghijk" titled "Song"
    And the download of the video "abcdefghijk" in "PL1" is running

  Scenario: A local check shows the saved state first and then the checked state
    When a local check is prepared
    Then the prepared watchlist has the stages:
      | source | item | stage    | status  |
      | PL1    | 1    | download | running |
    When the local check runs
    Then the published watchlist has the stages:
      | source | item | stage    | status      |
      | PL1    | 1    | download | not_started |
    And the saved watchlist has the stages:
      | source | item | stage    | status      |
      | PL1    | 1    | download | not_started |

  Scenario: A local check does nothing while a job runs
    Given a local check is prepared
    When the local check runs while a job runs
    Then the local check publishes nothing
    And the saved watchlist did not change

  Scenario: A local check is outdated when another check starts while it reads the settings
    Given a local check is prepared and another check starts while it reads the settings
    When the local check runs
    Then the local check publishes nothing
    And the saved watchlist did not change

  Scenario Outline: A local check that is outdated changes nothing
    Given a local check is prepared
    When <event>
    And the local check runs
    Then the local check publishes nothing
    And the saved watchlist did not change

    Examples:
      | event                                                  |
      | another local check is prepared                        |
      | another local check cannot read its settings           |
      | the source "PL1" is renamed to "Edited" by the user    |
      | a runner starts a job                                  |

  Scenario: A revision conflict loads the outside edit before the check saves
    Given a local check is prepared
    When the local check runs and another process renames the source "PL1" to "Edited by another process" at the third poll
    Then the published source "PL1" has the title "Edited by another process"
    And the saved source "PL1" has the title "Edited by another process"
    And the saved watchlist has the stages:
      | source | item | stage    | status      |
      | PL1    | 1    | download | not_started |

  Scenario: Repeated revision conflicts report an error and keep the outside edit
    Given a local check is prepared
    When the local check runs and another process renames the source "PL1" at each poll
    Then the local check reports "changed during the local check"
    And the saved source "PL1" has the title of the last outside edit
    And the saved watchlist has the stages:
      | source | item | stage    | status  |
      | PL1    | 1    | download | running |
