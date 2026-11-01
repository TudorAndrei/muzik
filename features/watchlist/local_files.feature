Feature: Watchlist state from local files
  muzik checks the saved watchlist against the downloaded files, the split
  folders, the cache of earlier versions, and the music library. A finished
  stage is marked complete. A video that is not finished leaves the processed
  list.

  Scenario: Finished stages are filled in and a waiting stage keeps its question
    Given the watched playlist "PL1" has the video "abcdefghijk" titled "Song"
    And the video "abcdefghijk" in "PL1" waits at the organize stage for an "import_match" question
    And the cache records the video "abcdefghijk" of "PL1" as "split" from the file "Song.flac" into the split folder "Song [abcdefghijk]"
    When the cache is imported and the watchlist is checked
    Then the checked watchlist has the stages:
      | source | item | stage    | status   |
      | PL1    | 1    | download | complete |
      | PL1    | 1    | split    | complete |
      | PL1    | 1    | organize | waiting  |
    And in the checked watchlist item 1 of "PL1" waits at the organize stage for an "import_match" question
    And the checked watchlist has no processed videos for "PL1"

  Scenario: An import that the cache left open fails once
    Given the watched playlist "PL1" has the video "abcdefghijk" titled "Song"
    And the video "abcdefghijk" in "PL1" was processed
    And the downloaded file "Song [abcdefghijk].flac" exists
    And the cache records the video "abcdefghijk" of "PL1" as "organized" from the file "Song [abcdefghijk].flac"
    When the cache is imported and the watchlist is checked
    Then the checked watchlist has no processed videos for "PL1"
    And the checked watchlist has the stages:
      | source | item | stage    | status |
      | PL1    | 1    | organize | failed |
    And item 1 of "PL1" in the checked watchlist has the last action "refresh"
    When the downloaded file "Song [abcdefghijk].flac" is deleted
    And the cache is imported again
    Then the cache import does nothing
    And the saved watchlist has the stages:
      | source | item | stage    | status |
      | PL1    | 1    | organize | failed |

  Scenario: A second check changes no rows
    Given the watched playlist "PL1" has the video "abcdefghijk" titled "Done"
    And the watched playlist "PL1" has the video "bcdefghijkl" titled "Downloaded"
    And the video "abcdefghijk" in "PL1" was processed
    And the download of the video "abcdefghijk" in "PL1" is running
    And the watchlist has the Spotify source "liked" with the track "t1"
    And the downloaded file "Song [bcdefghijkl].flac" exists
    When the watchlist is checked and saved 2 times
    And the watchlist is checked and saved 1 time
    Then the saved revision did not change
    And the saved watchlist has the stages:
      | source        | item | stage    | status   |
      | PL1           | 2    | download | complete |
      | spotify:liked | 1    | quality  | skipped  |

  Scenario: A kept source file with an empty split folder stays processed
    Given the watched playlist "PL1" has the video "abcdefghijk" titled "Song"
    And the video "abcdefghijk" in "PL1" was processed
    And the downloaded file "Song [abcdefghijk].flac" exists
    And the cache records the video "abcdefghijk" of "PL1" as "organized" from the file "Song [abcdefghijk].flac" into the split folder "Song [abcdefghijk]"
    When the cache is imported and the watchlist is checked
    Then the checked watchlist has the processed videos "abcdefghijk" for "PL1"
    And the checked watchlist has the stages:
      | source | item | stage    | status   |
      | PL1    | 1    | organize | complete |

  Scenario: A video that is already in the music library is complete
    Given the music library from the beets fixture
    And the watched playlist "PL1" has the video "video-123" titled "Unrelated title"
    And the watched playlist "PL1" has the video "other-video" titled "Artist - Album (Full Album)"
    When the watchlist is checked
    Then the checked watchlist has the processed videos "video-123, other-video" for "PL1"
    And the checked watchlist has the stages:
      | source | item | stage    | status   |
      | PL1    | 1    | download | complete |
      | PL1    | 1    | organize | complete |

  Scenario: Old cache entries for a YouTube video and a Spotify track are read
    Given the organize stage is turned off
    And the watched playlist "PL1" has the video "abcdefghijk" titled "Legacy"
    And the watchlist has the Spotify source "spotify:playlist:TEST" with the track "track-1"
    And the downloaded file "legacy.flac" exists
    And the old cache links the video "abcdefghijk" to the file "legacy.flac"
    And the cache records the track "track-1" of "spotify:playlist:TEST" as "downloaded" with the file "legacy.flac"
    When the cache is imported and the watchlist is checked
    Then the checked watchlist has the stages:
      | source                | item | stage    | status   |
      | PL1                   | 1    | download | complete |
      | spotify:playlist:TEST | 1    | download | complete |
      | spotify:playlist:TEST | 1    | quality  | skipped  |
      | spotify:playlist:TEST | 1    | organize | skipped  |
