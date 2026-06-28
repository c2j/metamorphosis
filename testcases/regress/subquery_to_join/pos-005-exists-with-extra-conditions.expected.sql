-- Extra condition preserved in ON clause; EXISTS removed
JOIN
u.status = 'active'
u.id = o.user_id
!EXISTS
