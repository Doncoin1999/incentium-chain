# The Alpha pool as decided on 26 Nov 2026: simulation

`cd sim && python3 alpha_decided.py` (full output below the summary is reproducible).

Rules simulated: ICT dividend to staked Alpha (validators' 38 % less a take, 18 % assumed);
no Alpha before it is issued; pool closed for the first 10 days while stock collects and the
treasury half of the auction price is held; opens as a plain x * y = k pool with real reserves at
the start price P0 from the valuation formula (r = 20 %, fully diluted), with
ICT = min(treasury half, 36,000 * P0) and Alpha = ICT / P0; stock released block by block, held
back while the price is more than 20 % under its 30-day average and released later (at most one
extra day a day); price limits instead of partial fills; 0.5 % fee, half burned.

| Scenario (10 subnets, 288 ICT a day each, r 20 %) | Opens with | Day 11 | Day 30 | Day 365 | ICT in pool, 1 y / 4 y | Return from day 366 |
| --- | --- | --- | --- | --- | --- | --- |
| Slot sold for 1,000 ICT | 500 ICT + 8,946 Alpha at 0.0559 | 0.0373 | 0.0468 | 0.0237 | 18,519 / 27,374 | 21.0 % a year |
| Slot sold for 10,000 ICT | 2,012 ICT + 36,000 Alpha (capped) | 0.0543 | 0.0519 | 0.0241 | 19,684 / 28,141 | 20.9 % |
| 30 subnets, 96 ICT a day each | 500 ICT + 26,838 Alpha at 0.0186 | 0.0164 | 0.0168 | 0.0080 | 6,450 / 9,307 | 20.9 % |

Findings:
- The design is consistent: stakers earn the required return (about 21 % at r = 20 %).
- A shallow start (small auction price) drops about a third on the first day as the ten closed
  days' bonus is sold, and recovers within weeks; a start with about 2,000 ICT drops 3 %.
- Panic (half of all holders sell at once on day 180): price falls 81 % that day and is back near
  value within 20 days while buyers price Alpha by its dividend.
- No buyers for 60 days: price falls to about 40 % of value; holding stock back keeps it about 4 %
  higher during the strike. Bonus selling, not stock, drives the fall; holding back is a small help.

Full output:

```

1. Base: slot sold for 1,000 ICT (500 ICT to the pool), 10 subnets, r 20 %
  start price P0 0.05589 ICT/Alpha; opened with 500 ICT and 8,946 Alpha; stock held back on 0 days
    day     price     value  ICT in pool  Alpha in pool  stock waiting   Alpha out  p.a. from d366
     10   0.00000   0.08235            0              0         36,000      36,000            nan%
     11   0.03725   0.07990          549         14,730         23,454      41,005            nan%
     30   0.04664   0.06121        2,529         54,226              0     161,608            nan%
     90   0.04009   0.04413        6,835        170,490              0     477,011            nan%
    180   0.03245   0.03396       11,686        360,164              0     934,838            nan%
    181   0.03238   0.03388       11,732        362,338              0     939,858            nan%
    200   0.03117   0.03247       12,590        403,885              0   1,035,006            nan%
    240   0.02893   0.02991       14,257        492,734              0   1,233,936            nan%
    270   0.02747   0.02828       15,399        560,522              0   1,381,981            nan%
    365   0.02370   0.02419       18,519        781,275              0   1,844,701            nan%
    730   0.01499   0.01511       25,785      1,720,241              0   3,531,712           22.3%
   1460   0.00646   0.00648       27,374      4,237,493              0   6,266,412           21.0%

2. Slot sold for 10,000 ICT (pool ICT capped at 36,000 x P0)
  start price P0 0.05589 ICT/Alpha; opened with 2,012 ICT and 36,000 Alpha; stock held back on 0 days
    day     price     value  ICT in pool  Alpha in pool  stock waiting   Alpha out  p.a. from d366
     10   0.00000   0.08754            0              0         36,000      36,000            nan%
     11   0.05426   0.08509        2,080         38,331              0      40,857            nan%
     30   0.05193   0.06387        3,793         73,042              0     142,792            nan%
     90   0.04161   0.04523        8,118        195,104              0     452,397            nan%
    180   0.03317   0.03458       12,927        389,738              0     905,264            nan%
    181   0.03310   0.03450       12,973        391,958              0     910,239            nan%
    200   0.03182   0.03304       13,822        434,352              0   1,004,539            nan%
    240   0.02948   0.03040       15,471        524,864              0   1,201,806            nan%
    270   0.02796   0.02872       16,601        593,816              0   1,348,687            nan%
    365   0.02407   0.02453       19,684        817,949              0   1,808,027            nan%
    730   0.01517   0.01529       26,829      1,768,421              0   3,483,532           22.1%
   1460   0.00652   0.00654       28,141      4,315,860              0   6,188,045           20.9%

3. Panic on day 180: half of all holders sell at once, stock held back
  start price P0 0.05589 ICT/Alpha; opened with 500 ICT and 8,946 Alpha; stock held back on 4 days
    day     price     value  ICT in pool  Alpha in pool  stock waiting   Alpha out  p.a. from d366
     10   0.00000   0.08250            0              0         36,000      36,000            nan%
     11   0.03730   0.08004          549         14,722         23,454      41,013            nan%
     30   0.04675   0.06136        2,534         54,201              0     161,633            nan%
     90   0.04023   0.04430        6,855        170,396              0     477,105            nan%
    180   0.03261   0.03413       11,734        359,865              0     935,137            nan%
    181   0.00616   0.03405        5,129        832,766              0     468,260            nan%
    200   0.03117   0.03248       12,594        404,094              0   1,033,627            nan%
    240   0.02895   0.02992       14,266        492,808              0   1,232,691            nan%
    270   0.02749   0.02829       15,408        560,607              0   1,380,725            nan%
    365   0.02371   0.02420       18,529        781,393              0   1,843,413            nan%
    730   0.01499   0.01512       25,796      1,720,467              0   3,530,315           22.3%
   1460   0.00646   0.00648       27,382      4,237,998              0   6,264,737           21.0%

4. Same panic without holding stock back
  start price P0 0.05589 ICT/Alpha; opened with 500 ICT and 8,946 Alpha; stock held back on 0 days
    day     price     value  ICT in pool  Alpha in pool  stock waiting   Alpha out  p.a. from d366
     10   0.00000   0.08247            0              0         36,000      36,000            nan%
     11   0.03729   0.08001          549         14,724         23,454      41,011            nan%
     30   0.04673   0.06133        2,533         54,206              0     161,628            nan%
     90   0.04020   0.04426        6,851        170,416              0     477,085            nan%
    180   0.03257   0.03410       11,724        359,929              0     935,073            nan%
    181   0.00615   0.03402        5,125        832,798              0     468,228            nan%
    200   0.03114   0.03245       12,529        402,373              0   1,035,347            nan%
    240   0.02892   0.02990       14,200        491,034              0   1,234,464            nan%
    270   0.02746   0.02827       15,343        558,770              0   1,382,563            nan%
    365   0.02369   0.02418       18,466        779,371              0   1,845,435            nan%
    730   0.01498   0.01511       25,739      1,717,814              0   3,532,969           22.3%
   1460   0.00646   0.00648       27,340      4,233,689              0   6,269,046           21.0%

6. No buyers from day 180 to 240, stock held back
  start price P0 0.05589 ICT/Alpha; opened with 500 ICT and 8,946 Alpha; stock held back on 23 days
    day     price     value  ICT in pool  Alpha in pool  stock waiting   Alpha out  p.a. from d366
     10   0.00000   0.08256            0              0         36,000      36,000            nan%
     11   0.03731   0.08010          549         14,718         23,454      41,017            nan%
     30   0.04680   0.06142        2,536         54,191              0     161,643            nan%
     90   0.04029   0.04436        6,863        170,359              0     477,142            nan%
    180   0.03267   0.03420       11,753        359,749              0     935,253            nan%
    181   0.03196   0.03412       11,682        365,561              0     936,635            nan%
    200   0.02307   0.03264       10,567        457,990         18,000     962,901            nan%
    240   0.01282   0.02963        9,036        704,872          3,600   1,018,197            nan%
    270   0.02716   0.02800       14,633        538,754              0   1,403,749            nan%
    365   0.02347   0.02397       17,775        757,327              0   1,868,649            nan%
    730   0.01487   0.01500       25,118      1,688,849              0   3,563,104           22.4%
   1460   0.00642   0.00644       26,883      4,186,573              0   6,317,332           21.0%

7. No buyers from day 180 to 240, stock released anyway
  start price P0 0.05589 ICT/Alpha; opened with 500 ICT and 8,946 Alpha; stock held back on 0 days
    day     price     value  ICT in pool  Alpha in pool  stock waiting   Alpha out  p.a. from d366
     10   0.00000   0.08257            0              0         36,000      36,000            nan%
     11   0.03732   0.08012          549         14,717         23,454      41,018            nan%
     30   0.04681   0.06144        2,537         54,188              0     161,646            nan%
     90   0.04030   0.04438        6,866        170,348              0     477,153            nan%
    180   0.03269   0.03422       11,759        359,714              0     935,288            nan%
    181   0.03197   0.03414       11,688        365,526              0     936,670            nan%
    200   0.02222   0.03266       10,578        475,955              0     962,936            nan%
    240   0.01284   0.02966        9,099        708,437              0   1,018,232            nan%
    270   0.02718   0.02802       14,686        540,256              0   1,402,248            nan%
    365   0.02349   0.02399       17,826        758,978              0   1,866,998            nan%
    730   0.01488   0.01501       25,164      1,691,012              0   3,560,941           22.4%
   1460   0.00642   0.00644       26,916      4,190,078              0   6,313,827           21.0%

5. 30 subnets (96 ICT a day each)
  start price P0 0.01863 ICT/Alpha; opened with 500 ICT and 26,838 Alpha; stock held back on 0 days
    day     price     value  ICT in pool  Alpha in pool  stock waiting   Alpha out  p.a. from d366
     10   0.00000   0.02856            0              0         36,000      36,000            nan%
     11   0.01636   0.02774          528         32,266          5,562      41,361            nan%
     30   0.01683   0.02103        1,140         67,721              0     148,113            nan%
     90   0.01373   0.01497        2,582        188,114              0     459,387            nan%
    180   0.01099   0.01147        4,190        381,323              0     913,679            nan%
    181   0.01096   0.01144        4,205        383,530              0     918,666            nan%
    200   0.01055   0.01096        4,489        425,682              0   1,013,210            nan%
    240   0.00977   0.01009        5,040        515,716              0   1,210,954            nan%
    270   0.00927   0.00953        5,418        584,334              0   1,358,169            nan%
    365   0.00799   0.00814        6,450        807,499              0   1,818,478            nan%
    730   0.00504   0.00508        8,843      1,754,678              0   3,497,275           22.2%
   1460   0.00217   0.00217        9,307      4,293,480              0   6,210,426           20.9%
```
