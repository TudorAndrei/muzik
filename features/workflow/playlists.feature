Feature: Playlists
  The workflow runs each item of a YouTube playlist or a Spotify export in
  order. A failed item does not stop later items. A new run continues from
  the items that are not complete.

  Scenario: A YouTube playlist keeps its order and continues after a failure
    Given the YouTube playlist has the videos "first, second, third"
    And the video "second" fails to download
    When the workflow runs the YouTube playlist "PL1"
    Then the playlist items are "complete, failed, complete"
    And the plan has 0 albums and 2 singles
    And YouTube downloaded the videos "first, second, third"

  Scenario: A YouTube playlist run continues from the failed item
    Given the YouTube playlist has the videos "first, second, third"
    And an earlier run of the YouTube playlist "PL1" failed at the video "second"
    When the workflow runs the YouTube playlist "PL1"
    Then the playlist items are "complete, complete, complete"
    And the plan has 0 albums and 1 single
    And YouTube downloaded the videos "second"

  Scenario: The main workflow sends a playlist URL to the playlist run
    Given the YouTube playlist has the videos "first"
    When the workflow runs for "https://www.youtube.com/playlist?list=PL1"
    Then the plan has 0 albums and 1 single
    And the playlist videos were listed

  Scenario: A Spotify export with a repeated track runs each position
    Given a Spotify JSON export with the same track at positions 1 and 2
    And Soulseek has the track audio
    When the workflow runs the Spotify export
    Then the playlist item ids are "spotify:track:same#0, spotify:track:same#1"
    And the plan has 0 albums and 2 singles
    And the track audio has the title "Two" and the artist "B"

  Scenario: A complete Spotify export run gets no audio again
    Given a Spotify JSON export with the same track at positions 1 and 2
    And Soulseek has the track audio
    And an earlier run of the Spotify export is complete
    When the workflow runs the Spotify export
    Then the plan has 0 albums and 0 singles
    And no Spotify track was acquired

  Scenario: A Spotify CSV export keeps quoted names
    Given a Spotify export "spotify.csv" with the content:
      """
      track_name,artist_name,spotify_track_id,position
      "One, Two",Artist,abc,1
      """
    When the Spotify export is read
    Then the first track has the title "One, Two"

  Scenario: A Spotify CSV export with a repeated position is rejected
    Given a Spotify export "spotify.csv" with the content:
      """
      track_name,artist_name,spotify_track_id,position
      One,Artist,abc,1
      Two,Artist,def,1
      """
    When the Spotify export is read
    Then the Spotify export is rejected

  Scenario: A Spotify JSON export with an invalid position is rejected
    Given a Spotify export "spotify.json" with the content:
      """
      {"version":1,"source":"spotify","type":"playlist","id":"p1","title":"List","entries":[{"title":"One","artist":"A","index":"wrong"}]}
      """
    When the Spotify export is read
    Then the Spotify export is rejected
