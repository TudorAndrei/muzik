Feature: Searching the library
  muzik understands beets queries. A query finds the same items and albums,
  in the same order, as beets finds.

  Background:
    Given the sample beets library

  Scenario Outline: A query finds the items that beets finds
    When I search the items for <query>
    Then the items are "<items>"

    Examples:
      | query                      | items |
      | artist:Artist              | 1     |
      | title::^T                  | 1     |
      | length:180..200            | 1     |
      | length:3:00..3:10          | 1     |
      | -artist:Artist             | 2     |
      | ^artist:Artist             | 2     |
      | title:"Track Name"         | none  |
      | artist:Artist, artist:Solo | 1, 2  |
      | title-                     | 1, 2  |
      | title+                     | 2, 1  |
      | title:Track artist:Artist  | 1     |
      | title:=Track               | 1     |
      | Artist                     | 1     |
      | muzik_source_id:video-456  | 2     |
      | album:Album                | 1     |

  Scenario Outline: A query finds the albums that match album fields
    When I search the albums for <query>
    Then the albums are "<albums>"

    Examples:
      | query       | albums |
      | album::^Alb | 1      |
      | album:Other | none   |

  Scenario: Items without a sort are in order of the artist sort name
    Given item 1 by "Alpha" with the artist sort name "Zulu"
    And item 2 by "Zulu" with the artist sort name "Alpha"
    When I sort the items with an empty query
    Then the items are "2, 1"

  Scenario: Albums without a sort are in order of the album artist sort name
    Given album 1 by "Alpha" with the album artist sort name "Zulu"
    And album 2 by "Zulu" with the album artist sort name "Alpha"
    When I sort the albums with an empty query
    Then the albums are "2, 1"
