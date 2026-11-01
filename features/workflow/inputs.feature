Feature: Workflow inputs
  The workflow accepts a local path, a YouTube video, a YouTube playlist,
  a Spotify export, or a search text. It finds the kind of input first.

  Scenario: A local Spotify CSV file is a Spotify export
    Given a Spotify CSV export
    When the workflow classifies the export
    Then the input is a Spotify export

  Scenario: A watch URL with a list is a playlist, not a video
    When the workflow classifies "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PL123"
    Then the input is the YouTube playlist "PL123"

  Scenario: A short link is a YouTube video
    When the workflow classifies "https://youtu.be/dQw4w9WgXcQ"
    Then the input is the YouTube video "dQw4w9WgXcQ"

  Scenario: Audio discovery follows links and finds each file once
    Given an album folder with one track, one text file, and a broken link
    And a link to the album folder
    When the workflow finds the audio in the album folder, its link, and a missing path
    Then it finds only the one track

  Scenario: Audio discovery stops at a link loop
    Given an album folder with one track, one text file, and a broken link
    And the album folder contains a link to itself
    When the workflow finds the audio in the album folder
    Then audio discovery fails
