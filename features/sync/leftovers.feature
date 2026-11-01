Feature: Old files on the device
  muzik treats media files that are not in the sync plan as old. Hidden
  files from macOS and from stopped syncs are old too. Other files stay.

  Scenario: Unplanned media and hidden leftovers are old
    Given the device has these files:
      | Album/01.mp3                 |
      | Album/02.flac                |
      | Album/._01.mp3               |
      | Album/.03.muzik-part.mp3     |
      | Album/.muzik-part.random.mp3 |
      | Album/notes.txt              |
    And the sync plan has "Album/01.mp3"
    When muzik looks for old files on the device
    Then the old files are:
      | Album/.03.muzik-part.mp3     |
      | Album/._01.mp3               |
      | Album/.muzik-part.random.mp3 |
      | Album/02.flac                |
