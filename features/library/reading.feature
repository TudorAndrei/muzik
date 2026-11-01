Feature: Reading the library
  muzik reads the items and albums of a beets library, with their fixed
  fields and their flexible fields.

  Background:
    Given the sample beets library

  Scenario: Items keep their fixed and flexible fields
    When I list the items
    Then the items are "1, 2"
    And item 1 has the field "title" set to "Track"
    And item 1 has the field "length" set to 183.5
    And item 1 has its path stored as bytes
    And item 1 is in album 1
    And item 1 has the flexible field "muzik_source_id" set to "video-123"
    And item 1 reads the same alone as in the list
    And item 2 is not in an album
    And item 2 has the flexible field "muzik_source_id" set to "video-456"

  Scenario: Albums keep their fields and their items
    When I list the albums
    Then the albums are "1"
    And album 1 has the field "album" set to "Album"
    And album 1 has the flexible field "fixture_note" set to "album-flex"
    And album 1 reads the same alone as in the list
    And album 1 holds the items "1"
