Feature: Cover files
  muzik saves album art next to the tracks, finds cover files in album
  folders, and reads the front cover that is in the track tags.

  Scenario Outline: Image data gets the file extension of its format
    Given <data> image data
    Then the image file extension is "<extension>"

    Examples:
      | data | extension |
      | JPEG | jpg       |
      | PNG  | png       |
      | GIF  | gif       |
      | WebP | webp      |
      | WAV  | none      |
      | HTML | none      |

  Scenario: A cover file is written once and never replaced
    Given JPEG image data
    When I write the cover file "cover"
    Then the cover file is "cover.jpg" with the image data
    When I write the cover file "cover"
    Then writing the cover file fails
    And the file "cover.jpg" still has the image data
    Given the data "not an image"
    When I write the cover file "cover"
    Then writing the cover file fails
    And the album folder holds only "cover.jpg"

  Scenario: A folder cover has the configured name or a usual name
    Given an empty album folder
    Then the album folder has no cover for the name "albumart"
    Given the album folder holds "albumart.webp"
    Then the album folder cover for the name "albumart" is "albumart.webp"
    Given the album folder holds "folder.jpg"
    Then the album folder cover for the name "albumart" is "folder.jpg"

  Scenario: The front cover comes from the first track that has one
    Given the track "00.flac" is not audio
    And the track "01.flac" has no cover
    And the track "02.flac" has a JPEG front cover
    Then the front cover of the tracks "00.flac, 01.flac, 02.flac" is the JPEG image
    And the tracks "00.flac, 01.flac" have no front cover
