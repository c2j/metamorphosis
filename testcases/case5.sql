insert into AAA
(SEQ_NO,
coin_code,
check_type,
tdstockbal)
select distinct p_i_seq_no,
p_i_sub_no,
v.coin_code,
(select b.check_type
from COR b
where b.label_code = v.label_code),
substr(v.class_value, -2),
v.tdstockbal
FROM PAR A, VAB v
where a.share_partner_code = v_share_partner_code
and a.fund_code = v.fund_code
AND v.accountdate = v_gffsrq
AND V_GFFSRQ BETWEEN A.INURE_BEGIN_DATE AND A.INURE_END_DATE
AND exists
(select 1
from PAR1 a
where a.account_type in ('01', '02')
and v.label_code = a.label_code)
AND substr(v.class_value, 1, 17) = substr(v_security_id, 1, 17)
and v.tdstockbal <> 0;
