Feature: Playlists on the device
  A sync copies the playlist files of the library to the device. Each entry
  points to the device file of its track. An entry for a track that is not on
  the device leaves the playlist. With delete on, a playlist that muzik wrote
  and that the library does not have now leaves the device. Other playlists
  on the device stay.

  Background:
    Given an "echo-mini" sync target

  Scenario: A device playlist points to the device files of its tracks
    Given the library has these tracks:
      | Artist/one.mp3 |
    And the library has the playlist "Mix.m3u8" with these entries:
      | ../Artist/one.mp3  |
      | ../Artist/gone.mp3 |
    When muzik prepares the sync with delete off
    And muzik applies the sync
    Then the device playlist "Mix.m3u8" has these entries:
      | ../Artist/one.mp3 |

  Scenario: An old muzik playlist leaves the device and other playlists stay
    Given the device has the muzik playlist "Old.m3u8"
    And the device has the playlist "Mine.m3u8"
    When muzik prepares the sync with delete on
    And muzik applies the sync
    Then the device does not have "Playlists/Old.m3u8"
    And the device still has "Playlists/Mine.m3u8"
