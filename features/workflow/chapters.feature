Feature: Chapter split
  The workflow splits an audio file with chapters into album tracks.
  Then it organizes the split folder.

  Scenario: A local album with chapters is split and then organized
    Given a local album "album.flac" with 2 chapters
    When the workflow runs for the local file
    Then the plan has 1 album and 0 singles
    And the source was split
    And the split folder "album" is organized

  Scenario: The YouTube description gives the chapters
    Given a local album "album.opus" with the YouTube description:
      """
      0:00 Opening
      3:12 Closing
      """
    And the metadata source is YouTube
    And organize is off
    When the workflow processes the local file
    Then the plan has 1 album and 0 singles
    And chapter 2 of the album is "Closing"
    And the source was split

  Scenario: Rejected chapters make the source a single
    Given a local album "album.flac" with 2 chapters
    And the reviewer rejects the chapters
    When the workflow runs for the local file
    Then the plan has 0 albums and 1 single
    And the reviewer saw the source
    And nothing was split
    And the source is organized

  Scenario: Edited chapters reach the split
    Given a local album "album.flac" with 2 chapters
    And the reviewer replaces the chapters with one chapter "Edited"
    When the workflow runs for the local file
    Then the album has only the chapter "Edited"
    And the split used only the chapter "Edited"

  Scenario: With no split, chaptered audio stays one import target
    Given a local album "album.flac" with 1 chapter
    When the workflow plans the local file with no split
    Then the plan has 0 albums and 1 single

  Scenario Outline: A forced split rejects a dot-only album name and keeps other files
    Given a local album "<name>" with 1 chapter
    And the split folder already has other files
    And force is on, with organize and metadata off
    And the real splitter is used
    When the workflow runs for the local file
    Then the workflow fails with "invalid album name"
    And the other files did not change
    And the source and its chapter file still exist

    Examples:
      | name    |
      | ...flac |
      | ..flac  |

  Scenario: A better-quality album replaces the source without a chapter split
    Given a local file "source.flac"
    And the quality check replaces the source with an album folder
    When the workflow runs for the local file
    Then the quality check ran once
    And nothing was split
    And the replacement album folder is organized
    And the replacement album folder is the split result
    And the source still exists
