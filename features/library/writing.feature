Feature: Writing to the library
  muzik adds, changes and removes items and albums in a beets library. A
  group of writes is all or nothing, and muzik keeps a backup of the library.

  Scenario: The first write makes a new library
    Given there is no library file yet
    Then a library in memory has no items
    When I add the album "First" with the track "Track"
    Then the library file exists
    And a reader of the library file sees 1 item in the album

  Scenario: Fixed and flexible fields are written in one group
    Given a copy of the sample beets library
    When one group of writes adds the album "New Album" with the review "pending", adds its track "New Track" from the source "source-123", and renames the track to "Changed"
    Then the track in the album has the field "title" set to "Changed"
    And the track in the album has the flexible field "muzik_source_id" set to "source-123"
    And the album has the flexible field "review" set to "pending"
    And a backup of the library exists
    When I remove the track in the album
    Then the album is removed too

  Scenario: A failed group of writes changes nothing
    Given a copy of the sample beets library
    When one group of writes adds the album "Rollback" and then an item with the unknown field "not_a_beets_field"
    Then the group of writes fails because of an invalid field
    And the library has 1 album

  Scenario: Pruning stops before it removes too many items
    Given a copy of the sample beets library
    And a music folder that holds the file of a new item
    When I prune the missing items with a safety fraction of 0.5
    Then pruning stops because 2 of 3 items are missing
    And the library has 3 items

  Scenario: Pruning removes the items whose files are missing
    Given a copy of the sample beets library
    And a music folder that holds the file of a new item
    When I prune the missing items with a safety fraction of 1.0
    Then 2 items are pruned
    And the library has 1 item
    And the item that remains has the file in the music folder
