Feature: Watchlist refresh
  A refresh reads each watched source, adds new videos, and processes each
  video that is not finished. A sync only reads the sources and lists the
  videos that wait for processing. Both keep the saved state of each item.

  Scenario: A refresh keeps earlier state and processes each video once
    Given the watched playlist "PL123" has the videos "video_a"
    And the video "video_a" in "PL123" was last retried
    And the source now lists the videos "video_a, video_a, video_b" with the title "Current title"
    When the watchlist is refreshed
    Then the videos "video_a, video_b" were processed
    And the watchlist was saved 3 times
    And the refresh summary counts 2 completed videos
    And the refreshed source "PL123" has the title "Current title"
    And the refreshed watchlist has the stages:
      | source | item | stage    | status   |
      | PL123  | 2    | download | complete |
    And the saved watchlist has the processed videos "video_a, video_b" for "PL123"

  Scenario: A video that waits for an answer does not block the refresh
    Given the watched playlist "PL123" has no videos
    And the source now lists the videos "video_a, video_b"
    And the video "video_a" asks an "import_match" question at the organize stage
    When the watchlist is refreshed
    Then the videos "video_a, video_b" were processed
    And the refresh summary counts 1 waiting video
    And the refresh summary counts 0 failed videos
    And in the saved watchlist item 1 of "PL123" waits at the organize stage for an "import_match" question
    And the refreshed card of item 1 in "PL123" reads "Waiting"
    When the watchlist is refreshed
    Then no video was processed
    And the saved watchlist has the stages:
      | source | item | stage    | status  |
      | PL123  | 1    | organize | waiting |

  Scenario: A refresh reads every source before it processes items
    Given the watchlist has the source "https://www.youtube.com/playlist?list=PLone"
    And the watchlist has the source "https://www.youtube.com/playlist?list=PLtwo"
    And each source lists one video named after the source
    And processing leaves each video as it is
    When the watchlist is refreshed
    Then the work was "load PLone, load PLtwo, process video_PLone, process video_PLtwo"

  Scenario: A refresh of one source reads and processes only that source
    Given the watchlist has the source "https://www.youtube.com/playlist?list=PLone"
    And the watchlist has the source "https://www.youtube.com/playlist?list=PLtwo"
    And each source lists one video named after the source
    And processing leaves each video as it is
    When the source "PLtwo" is refreshed
    Then the work was "load PLtwo, process video_PLtwo"

  Scenario: A refresh of a source that is not watched fails
    Given the watchlist has the source "https://www.youtube.com/playlist?list=PLone"
    And the watchlist has the source "https://www.youtube.com/playlist?list=PLtwo"
    And each source lists one video named after the source
    When the source "PLgone" is refreshed
    Then the job fails

  Scenario: A cancelled refresh keeps the videos saved before the stop
    Given the watched playlist "PL123" has no videos
    And the source now lists the videos "video_a, video_a, video_b" with the title "Current title"
    And the job is cancelled while the second video is processed
    When the watchlist is refreshed
    Then the job is cancelled
    And the saved watchlist has the processed videos "video_a" for "PL123"

  Scenario: A dry run changes nothing and processes no audio
    Given the watched playlist "PL123" has the videos "video_a"
    And the source now lists the videos "video_a, video_a, video_b" with the title "Current title"
    And jobs are dry runs
    When the watchlist is refreshed
    Then the refresh summary counts 2 pending videos
    And the refresh summary counts 0 completed videos
    And the saved watchlist did not change
    When the action "download_again" runs on the video "video_a" at position 1 of "PL123"
    Then the action reports a dry run
    And the saved watchlist did not change
    And no video was processed

  Scenario: A sync lists pending videos and keeps running stages
    Given the watched playlist "PL123" has the videos "video_a"
    And the download of the video "video_a" in "PL123" is running
    And the source now lists the videos "video_a, video_a, video_b" with the title "Current title"
    When the watchlist is synced
    Then no video was processed
    And the pending videos of "PL123" are "video_a at 1, video_b at 3"
    And the saved watchlist has the stages:
      | source | item | stage    | status  |
      | PL123  | 1    | download | running |

  Scenario: A private video is not queued and shows as unavailable
    Given the watched playlist "PL123" has no videos
    And the source now lists the videos "video_a, video_b"
    And the listed video "video_b" is private
    When the watchlist is synced
    Then the pending videos of "PL123" are "video_a at 1"
    When the cards are shown
    Then the shown card of item 2 in "PL123" reads "Unavailable"
    And the shown card of item 2 in "PL123" has no main action
    And the shown card of item 2 in "PL123" does not allow the action "retry"
