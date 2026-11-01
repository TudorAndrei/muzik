Feature: Incremental import
  With incremental import on, muzik remembers which folders it imported and
  does not import them again.

  Background:
    Given MusicBrainz has the release "Night Lines" by "Mara Vale" from 2021 with the id "new-release":
      | track | title              | artist    | recording   | release track   |
      | 2     | Song (feat. Guest) | Mara Vale | recording-1 | release-track-1 |
    And the incoming song "02 Song.flac" that muzik knows as "Song" by "Mara Vale" on "Night Lines" from 2021

  Scenario: An imported folder is added to the history
    Given incremental import is on with an empty history
    And autotag is off
    When muzik plans the import of the songs
    And muzik imports the album as it is and keeps duplicates
    Then no history update failed
    And the history has the incoming folder

  Scenario Outline: A skipped folder is added to the history unless skip later is on
    Given incremental import is on with an empty history
    And incremental skip later is <skip later>
    And autotag is off
    When muzik plans the import of the songs
    And muzik skips the album
    Then the import skipped 1 album
    And the history <result> the incoming folder

    Examples:
      | skip later | result            |
      | off        | has               |
      | on         | does not have     |

  Scenario: A dry run does not write the history
    Given incremental import is on with an empty history
    And autotag is off
    And dry run is on
    When muzik plans the import of the songs
    And muzik imports the album as it is and keeps duplicates
    Then the history file is not written

  Scenario: A folder from the history is skipped, and a real run saves the history
    Given the incremental history already has the incoming folder
    When muzik plans the import of the songs
    Then MusicBrainz was not asked
    And the plan has 0 albums
    And the plan skipped 1 folder imported before
    And the history file is not written
    When muzik runs the plan
    Then the import skipped 1 folder imported before
    And the history file is written
    And the history has the incoming folder
