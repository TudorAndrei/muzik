Feature: Watchlist playlist files
  With playlist files on, muzik writes one file for each watchlist source to
  the Playlists folder of the music library. The file lists the library songs
  of the source in source order. Each source has its own switch. A file that
  muzik wrote leaves the folder when its source is off or when playlist files
  are off. Other files in the folder stay.

  Background:
    Given a music library with these songs:
      | path              | artist | title | album |
      | A/One/01 One.flac | Alex   | One   | One   |
      | B/Two/01 Two.flac | Blair  | Two   | Two   |
    And the watchlist has the liked songs with these tracks:
      | artist | title   |
      | Blair  | Two     |
      | Alex   | One     |
      | Casey  | Missing |

  Scenario: A playlist lists the library songs in source order
    When muzik writes the playlist files as "m3u8"
    Then the playlist "Liked Songs.m3u8" has these entries:
      | ../B/Two/01 Two.flac |
      | ../A/One/01 One.flac |
    And 1 item of "spotify:liked" is not in the library

  Scenario: The m3u format changes the file extension
    When muzik writes the playlist files as "m3u"
    Then the playlist folder has only "Liked Songs.m3u"

  Scenario: A source with its playlist off loses its file and other files stay
    Given the playlist folder has the file "Mine.m3u8"
    And muzik wrote the playlist files as "m3u8"
    When the playlist file of "spotify:liked" is turned off
    And muzik writes the playlist files as "m3u8"
    Then the playlist folder has only "Mine.m3u8"

  Scenario: With playlist files off, muzik removes only its own files
    Given the playlist folder has the file "Mine.m3u8"
    And muzik wrote the playlist files as "m3u8"
    When muzik writes the playlist files with playlist files off
    Then the playlist folder has only "Mine.m3u8"
