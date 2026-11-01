Feature: Device formats
  Each sync preset decides if muzik copies a track as it is or converts it
  to a format that the device plays.

  Scenario Outline: The Echo Mini preset keeps audio that it plays and converts the rest
    Given an "echo-mini" sync target
    When muzik checks a <format> track at <rate> Hz with <depth> bit depth
    Then the track is <result>

    Examples:
      | format | rate   | depth | result                                    |
      | flac   | 44100  | 24    | copied                                    |
      | mp3    | 44100  | no    | copied                                    |
      | vorbis | 44100  | no    | copied                                    |
      | opus   | 48000  | no    | converted to MP3 at 320 kbps              |
      | flac   | 352800 | 32    | converted to FLAC at 176400 Hz and 24 bit |

  Scenario Outline: The Opus preset converts lossless audio and keeps common lossy audio
    Given an "opus" sync target with a bitrate of 128 kbps
    When muzik checks a <format> track at <rate> Hz with <depth> bit depth
    Then the track is <result>

    Examples:
      | format | rate  | depth | result                        |
      | flac   | 44100 | 16    | converted to Opus at 128 kbps |
      | aac    | 44100 | no    | copied                        |
      | opus   | 48000 | no    | copied                        |

  Scenario Outline: The MP3 preset converts all audio except MP3
    Given an "mp3" sync target
    When muzik checks a <format> track at <rate> Hz with <depth> bit depth
    Then the track is <result>

    Examples:
      | format | rate  | depth | result                       |
      | flac   | 44100 | 16    | converted to MP3 at 320 kbps |
      | opus   | 44100 | 16    | converted to MP3 at 320 kbps |
      | aac    | 44100 | 16    | converted to MP3 at 320 kbps |
      | vorbis | 44100 | 16    | converted to MP3 at 320 kbps |
      | mp3    | 44100 | no    | copied                       |
