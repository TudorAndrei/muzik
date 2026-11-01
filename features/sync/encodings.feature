Feature: Converted files on the device
  muzik records the encoding of each file that it converts. A converted file
  is current only when the record matches the encoding that the target needs.

  Background:
    Given an "mp3" sync target
    And the library has these tracks:
      | Album/01 Song.flac |
    And the converted file is on the device

  Scenario: A converted file with no record is converted again
    When muzik plans the sync
    Then the plan has 0 current files

  Scenario: A converted file with a matching record is current
    Given muzik recorded the encoding of the converted file
    When muzik plans the sync
    Then the plan has 1 current file

  Scenario: A new bitrate converts the file again
    Given muzik recorded the encoding of the converted file
    And the target bitrate is 128 kbps
    When muzik plans the sync
    Then the plan converts the file to MP3 at 128 kbps

  Scenario: A plain copy removes the recorded encoding
    Given muzik recorded the encoding of the converted file
    When muzik records the file as a plain copy
    Then the device has no recorded encodings
