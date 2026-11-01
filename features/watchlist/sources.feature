Feature: Watchlist sources
  The user adds YouTube playlists, Spotify playlists, albums, liked songs, and
  the Bandcamp collection to the watchlist. The user can rename and remove a
  source. A change to a source keeps the state of its items.

  Scenario Outline: A link from a supported service is accepted
    When the link "<link>" is read as a source
    Then the source has the ID "<id>"

    Examples:
      | link                                                       | id                     |
      | https://music.youtube.com/watch?v=abc&list=PL_ONE#tracks   | PL_ONE                 |
      | http://m.youtube.com/playlist?list=PL_TWO                  | PL_TWO                 |
      | https://youtu.be/video?list=PL_THREE                       | PL_THREE               |
      | https://open.spotify.com/intl-de/album/AL1/?si=abc         | spotify:album:AL1      |
      | spotify:playlist:PL1                                       | spotify:playlist:PL1   |
      | Liked Songs                                                | spotify:liked          |

  Scenario Outline: A link from another host is rejected
    When the link "<link>" is read as a source
    Then the source is rejected

    Examples:
      | link                                                 |
      | https://example.test/playlist?list=PL1               |
      | https://example.test/open.spotify.com/playlist/PL1   |
      | https://open.spotify.com.example.test/playlist/PL1   |
      | ftp://youtube.com/playlist?list=PL1                  |
      | http://open.spotify.com/playlist/PL1                 |

  Scenario: A YouTube video link adds the playlist of the video
    When the source "https://www.youtube.com/watch?v=abcdefghijk&list=PL_ONE" is added
    Then the source has the ID "PL_ONE"
    And the source has the link "https://www.youtube.com/playlist?list=PL_ONE"

  Scenario: The same playlist cannot be added twice
    Given the watchlist has the source "https://www.youtube.com/watch?v=abcdefghijk&list=PL_ONE"
    When the source "https://youtube.com/playlist?list=PL_ONE" is added
    Then the source is rejected

  Scenario Outline: Spotify sources get a Spotify ID
    When the source "<link>" is added
    Then the source has the ID "<id>"

    Examples:
      | link                                                          | id                                      |
      | liked                                                         | spotify:liked                           |
      | https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=1 | spotify:playlist:37i9dQZF1DXcBWIGoYBM5M |

  Scenario: Liked songs get their own title
    When the source "liked" is added
    Then the source has the title "Liked Songs"

  Scenario: Renaming a source keeps the state of its items
    Given the watchlist has the source "https://www.youtube.com/watch?v=abcdefghijk&list=PL_ONE"
    And the video "abcdefghijk" in "PL_ONE" was downloaded to "/music/song.flac"
    When the source "PL_ONE" is renamed to " Jazz albums "
    Then the change is made
    And the saved source "PL_ONE" has the title "Jazz albums"
    And item 1 of "PL_ONE" in the saved watchlist has the download path "/music/song.flac"

  Scenario: Removing a source keeps the other sources
    Given the watchlist has the source "https://www.youtube.com/watch?v=abcdefghijk&list=PL_ONE"
    And the watchlist has the source "liked"
    And the watchlist has the source "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=1"
    When the source "spotify:liked" is removed
    Then the change is made
    And the saved watchlist has 2 sources

  Scenario: The Bandcamp collection is added once
    Given the Bandcamp collection of "listener" was added
    When the Bandcamp collection of "listener" is added
    Then nothing is changed
    And the saved watchlist did not change
    And the saved watchlist has 1 source
    And the saved source "bandcamp:collection" is a "bandcamp" source with the link "https://bandcamp.com/listener"
