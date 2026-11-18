Feature: Watchlist item actions
  The user runs an action on one watchlist item, for example run, retry, or
  download again. A question pauses the item at the stage that asked it. A
  failure is saved on the stage that failed.

  Scenario Outline: Each question pauses the item at its own stage
    When a video asks an "<question>" question
    Then the video waits at the <stage> stage

    Examples:
      | question            | stage    |
      | import_match        | organize |
      | import_duplicate    | organize |
      | chapter_review      | parse    |
      | chapter_edit        | parse    |
      | quality_replacement | quality  |
      | soulseek_candidate  | download |

  Scenario: An action that needs a choice parks the item
    Given the watched playlist "PL123" has the videos "video_a"
    And the video "video_a" asks an "import_match" question at the organize stage
    When the action "run" runs on the video "video_a" of "PL123"
    Then the action waits at the organize stage
    And in the saved watchlist item 1 of "PL123" waits at the organize stage for an "import_match" question

  Scenario: A failed action saves the stage of the action
    Given the watched playlist "PL123" has the videos "video_a"
    And processing fails with the error "download failed"
    When the action "run" runs on the video "video_a" of "PL123"
    Then the job fails
    And the saved watchlist has the stages:
      | source | item | stage    | status |
      | PL123  | 1    | download | failed |
    And item 1 of "PL123" in the saved watchlist has the error "download failed"

  Scenario: An action checks the identity of the saved item
    Given the watched playlist "PL123" has the videos "video_a"
    When the action "run" runs on the video "other" of "PL123"
    Then the job fails
    And no video was processed

  Scenario: An action finds its item after a sync moved it
    Given the watched playlist "PL123" has the videos "video_a"
    And the source now lists the videos "video_b, video_a"
    And processing fails with the error "download failed"
    When the watchlist is synced
    And the action "run" runs on the video "video_a" of "PL123"
    Then the job fails
    And the saved watchlist has the stages:
      | source | item | stage    | status      |
      | PL123  | 1    | download | not_started |
      | PL123  | 2    | download | failed      |
    And item 2 of "PL123" in the saved watchlist has the error "download failed"

  Scenario: A failure marks the stage that failed
    Given the watched playlist "PL123" has the videos "video_a"
    And the split stage fails with the error "split failed"
    When the item "video_a" of "PL123" runs
    Then the item fails at a stage
    And the saved watchlist has the stages:
      | source | item | stage    | status      |
      | PL123  | 1    | split    | failed      |
      | PL123  | 1    | download | not_started |
    And the split stage of item 1 of "PL123" in the saved watchlist has the error "split failed"

  Scenario: Downloading again keeps later stages stale after a check of the cache
    Given the watched playlist "PL123" has the videos "video_a, video_a"
    And the video "video_a" in "PL123" was processed
    And the cache records the video "video_a" of "PL123" as "organized" from the file "old.flac"
    And processing downloads the video again
    When the action "download_again" runs on the video "video_a" of "PL123"
    And the watchlist is checked
    Then the checked watchlist has no processed videos for "PL123"
    And the checked items of "PL123" are at the positions "1, 2"
    And the checked watchlist has the stages:
      | source | item | stage    | status   |
      | PL123  | 1    | download | complete |
      | PL123  | 1    | organize | stale    |
      | PL123  | 2    | download | complete |
      | PL123  | 2    | organize | stale    |

  Scenario: A question after the download keeps the download
    Given the watched playlist "PL123" has the videos "video_a"
    And processing saves the download before the later stages
    And the video "video_a" asks an "import_match" question at the organize stage
    When the action "run" runs on the video "video_a" of "PL123"
    Then the action waits at the organize stage
    And the saved watchlist has the stages:
      | source | item | stage    | status   |
      | PL123  | 1    | download | complete |
      | PL123  | 1    | organize | waiting  |
    And item 1 of "PL123" in the saved watchlist has the download path "video_a.opus"

  Scenario: A failure after the download keeps the download
    Given the watched playlist "PL123" has the videos "video_a"
    And processing saves the download before the later stages
    And the split stage fails with the error "split failed"
    When the item "video_a" of "PL123" runs
    Then the item fails at a stage
    And the saved watchlist has the stages:
      | source | item | stage    | status   |
      | PL123  | 1    | download | complete |
      | PL123  | 1    | split    | failed   |
    And item 1 of "PL123" in the saved watchlist has the download path "video_a.opus"

  Scenario: A cancel after the download keeps the download
    Given the watched playlist "PL123" has the videos "video_a"
    And processing saves the download before the later stages
    And the job is cancelled while the video is processed
    When the action "run" runs on the video "video_a" of "PL123"
    Then the job is cancelled
    And the saved watchlist has the stages:
      | source | item | stage    | status      |
      | PL123  | 1    | download | complete    |
      | PL123  | 1    | organize | not_started |
    And item 1 of "PL123" in the saved watchlist has the download path "video_a.opus"

  Scenario: A result keeps the title that a sync saved during the run
    Given the watched playlist "PL123" has the video "video_a" titled "Old title"
    And a sync renames the video "video_a" to "New title" while it is processed
    And the video "video_a" asks an "import_match" question at the organize stage
    When the action "run" runs on the video "video_a" of "PL123"
    Then the action waits at the organize stage
    And item 1 of "PL123" in the saved watchlist has the title "New title"

  Scenario: A video that leaves its playlist during the run fails the job
    Given the watched playlist "PL123" has the videos "video_a, video_b"
    And a sync removes the video "video_a" while it is processed
    And the video "video_a" asks an "import_match" question at the organize stage
    When the action "run" runs on the video "video_a" of "PL123"
    Then the job fails
    And the saved source "PL123" has 1 item

  Scenario: A choice that cannot resume does not keep the item waiting
    Given the watched playlist "PL123" has the videos "video_a"
    And the video "video_a" in "PL123" waits at the organize stage for an "import_match" question
    When the action "organize_again" runs on the video "video_a" of "PL123"
    Then the job fails
    And no video was processed
    And the video "video_a" of "PL123" has no open job
    And the saved watchlist has the stages:
      | source | item | stage    | status |
      | PL123  | 1    | organize | failed |
    And item 1 of "PL123" in the saved watchlist has the error "No downloaded audio or split directory is available."

  Scenario: A command that cannot run keeps a complete stage
    Given the watched playlist "PL123" has the videos "video_a"
    When the action "run" runs on the video "video_a" of "PL123"
    And the action "organize_again" runs on the video "video_a" of "PL123"
    Then the job fails
    And the saved watchlist has the stages:
      | source | item | stage    | status   |
      | PL123  | 1    | organize | complete |
    And the saved watchlist has 1 processed videos for "PL123"

  Scenario: A job that ends before it starts keeps a complete stage
    Given the watched playlist "PL123" has the videos "video_a"
    When the action "run" runs on the video "video_a" of "PL123"
    And a "download_again" job of the video "video_a" of "PL123" ends before it starts with the error "settings are invalid"
    Then the video "video_a" of "PL123" has no open job
    And the saved watchlist has the stages:
      | source | item | stage    | status   |
      | PL123  | 1    | download | complete |
    And the saved watchlist has 1 processed videos for "PL123"

  Scenario: A job that ends before it starts does not keep the item waiting
    Given the watched playlist "PL123" has the videos "video_a"
    And the video "video_a" in "PL123" waits at the organize stage for an "import_match" question
    When the job of the video "video_a" of "PL123" ends before it starts with the error "settings are invalid"
    Then the video "video_a" of "PL123" has no open job
    And the saved watchlist has the stages:
      | source | item | stage    | status |
      | PL123  | 1    | organize | failed |
    And item 1 of "PL123" in the saved watchlist has the error "settings are invalid"
