Feature: Audio sources
  The workflow gets audio from YouTube or Soulseek. It can fall back from
  Soulseek to YouTube when Soulseek has no result.

  Scenario: A video that is already downloaded is not downloaded again
    Given the download folder has the audio of the video "dQw4w9WgXcQ"
    When the workflow runs for "https://youtu.be/dQw4w9WgXcQ"
    Then nothing is downloaded from YouTube
    And the downloaded audio is a single
    And the downloaded audio is organized

  Scenario: A dry run plans the work but changes no files
    Given dry run is on
    When the workflow runs for "https://youtu.be/dQw4w9WgXcQ"
    Then nothing is downloaded from YouTube
    And nothing is organized
    And there are no organize targets

  Scenario: Soulseek is the audio source for a search
    Given the audio source is Soulseek
    And Soulseek has a result
    When the workflow runs for "Artist - Song"
    Then the Soulseek result is the organize target
    And nothing is downloaded from YouTube

  Scenario: A cancelled workflow does not import
    Given the audio source is Soulseek
    And a local file "song.flac"
    And the workflow is cancelled
    When the workflow runs for the local file
    Then the workflow is cancelled
    And nothing is organized

  Scenario: A video comes from Soulseek when Soulseek has a result
    Given the audio source is Soulseek with a YouTube fallback
    And organize is off
    And Soulseek has a result
    When the workflow runs for "https://youtu.be/dQw4w9WgXcQ"
    Then Soulseek was searched for "https://youtu.be/dQw4w9WgXcQ"
    And nothing is downloaded from YouTube

  Scenario Outline: YouTube is the fallback when Soulseek has no result
    Given the audio source is Soulseek with a YouTube fallback
    And organize is off
    And YouTube has a download
    When the workflow runs for "<input>"
    Then Soulseek was searched for "<input>"
    And YouTube downloaded "<input>"

    Examples:
      | input                        |
      | https://youtu.be/dQw4w9WgXcQ |
      | Artist - Song                |

  Scenario: Cancellation during the Soulseek search stops the fallback
    Given the audio source is Soulseek with a YouTube fallback
    And the workflow is cancelled during the Soulseek search
    When the workflow runs for "Artist - Song"
    Then the workflow is cancelled
    And Soulseek was searched for "Artist - Song"
    And nothing is downloaded from YouTube
