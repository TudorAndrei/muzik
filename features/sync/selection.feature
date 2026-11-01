Feature: Selecting library tracks to sync
  A library query selects the tracks to sync. With covers on, the album
  cover files go to the device with the tracks.

  Background:
    Given a library with the album "SelectRun" that has the track "Artist/song.mp3" and a cover file

  Scenario: With covers on, muzik selects the tracks and the album cover
    When muzik selects "album:SelectRun" with covers
    Then the selected tracks are:
      | Artist/song.mp3 |
    And the album cover is selected

  Scenario: With covers off, muzik selects no covers
    When muzik selects "album:SelectRun" without covers
    Then no covers are selected
