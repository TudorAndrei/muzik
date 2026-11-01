Feature: Watchlist cards
  Each watchlist item shows as a card with a summary, a main action, the
  actions that the user can take, and a cached thumbnail. Showing the cards
  does not change the saved watchlist.

  Scenario: A new video shows the run action and its cached thumbnail
    Given the watched playlist "PL1" has the video "abcdefghijk" titled "Song"
    And the source "PL1" and its items have fields from a newer version of muzik
    And the cache has the thumbnail "yt_thumbnail_abcdefghijk.jpg"
    When the cards are shown
    Then the shown source "PL1" and its items keep the fields from the newer version
    And the shown card of item 1 in "PL1" reads "Pending"
    And the shown card of item 1 in "PL1" has the main action "run" labelled "Run"
    And the shown card of item 1 in "PL1" allows the action "run"
    And the shown card of item 1 in "PL1" does not allow the action "split_again"
    And the shown card of item 1 in "PL1" has the thumbnail "yt_thumbnail_abcdefghijk.jpg" from the cache
    And the saved watchlist did not change

  Scenario Outline: A removed or private video is not shown as failed
    Given the watched playlist "PL1" has the video "<video>" titled "<video>"
    And the download of the video "<video>" in "PL1" failed with the error "<error>"
    When the cards are shown
    Then the shown card of item 1 in "PL1" reads "Unavailable"
    And the shown card of item 1 in "PL1" has no main action
    And the shown card of item 1 in "PL1" does not allow the action "retry"

    Examples:
      | video       | error                                                                                                                       |
      | aaaaaaaaaaa | yt-dlp failed: ERROR: [youtube] aaaaaaaaaaa: Private video                                                                  |
      | bbbbbbbbbbb | yt-dlp failed: WARNING: [youtube] unable to extract yt initial data\nERROR: [youtube] bbbbbbbbbbb: Video unavailable        |

  Scenario: A video that failed for another reason can be retried
    Given the watched playlist "PL1" has the video "ccccccccccc" titled "ccccccccccc"
    And the download of the video "ccccccccccc" in "PL1" failed with the error "yt-dlp failed: ERROR: unable to download video data: HTTP Error 403: Forbidden"
    When the cards are shown
    Then the shown card of item 1 in "PL1" reads "Failed"
    And the shown card of item 1 in "PL1" has the main action "retry"
